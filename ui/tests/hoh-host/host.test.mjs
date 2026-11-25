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
    threads: await built.load('threads'), host: await built.load('host'), wire: await built.load('wire'), socket: await built.load('socket'),
    http: await built.load('http'), config: await built.load('config'), index: await built.load('index'), errors: await built.load('errors'),
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

test('changed-folder refusal is explained without exposing an engine error', () => {
  const failure = mod.errors.providerFailure({ code: 'folder-changed', message: 'internal error -32603' }, 'transport_error', 'Failed');
  assert.match(failure.message, /folder for this chat has changed/i);
  assert.match(failure.message, /rebind/i);
  assert.doesNotMatch(failure.message, /-32603/);
});

test('handshake: config snapshot, probe, settings, ping; Claude and Codex are offered', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  const config = await client.stream('subscribeServerConfig', { environmentThemes: true, workspaceSnapshotsOnDemand: true });
  const first = await config.until(value => value.type === 'snapshot'
    && value.config.providers[0].models.some(model => model.slug === 'claude-sonnet')
    && value.config.providers[1].models.some(model => model.slug === 'gpt-6-sol'));
  assert.equal(first.version, 1);
  assert.equal(first.config.environment.environmentId, 'hoh-local');
  assert.equal(first.config.environment.orchestrationProtocolVersion, 2);
  assert.deepEqual(first.config.providers.map(provider => provider.instanceId), ['claudeAgent', 'codex']);
  assert.deepEqual(first.config.providers[0].models.map(model => model.slug), ['claude-sonnet', 'claude-opus']);
  assert.equal(first.config.providers[0].status, 'ready');
  assert.deepEqual(first.config.providers[0].supportedRuntimeModes, ['approval-required', 'auto-accept-edits', 'auto', 'full-access']);
  assert.deepEqual(first.config.providers[1].models.map(model => model.slug), ['gpt-6-sol']);
  assert.deepEqual(first.config.providers[1].supportedRuntimeModes, ['approval-required', 'auto-accept-edits', 'auto', 'full-access']);
  assert.deepEqual(first.config.settings.defaultModelSelection, { instanceId: 'claudeAgent', model: 'claude-sonnet' });
  assert.equal(first.config.settings.defaultRuntimeMode, 'full-access');
  // Nothing optional is advertised: T3 hides git, terminal, preview, PRs, issues… by their absence.
  assert.deepEqual(Object.keys(first.config.environment.capabilities).sort(), ['connectionProbe', 'inlineMessageContext', 'providerWorkspaceSnapshotsOnDemand', 'repositoryIdentity', 'threadForceStop']);
  assert.deepEqual(await client.call('server.probe', {}), {});
  assert.deepEqual(await client.request('server.reportClientActivity', {}), { _tag: 'Success', value: null });
  assert.equal((await client.call('server.getConfig', {})).environment.label, 'HoH');
  assert.equal((await client.call('server.getSettings', {})).defaultRuntimeMode, 'full-access');
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

test('a built-in agent session remains in the inventory when it has no external binding', async () => {
  const engine = new FakeEngine();
  engine.seed('built-in', 'Built-in conversation', []);
  const original = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && params.sessionId === 'built-in')
      return { session: { executor: null, runtimeMode: null, builtin: true } };
    return original(method, params);
  };
  const { client, host } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  const snapshot = await shell.until(value => value.kind === 'snapshot');
  assert(snapshot.snapshot.threads.some(thread => thread.id === 'built-in'));
  await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'builtin-send', threadId: 'built-in', messageId: 'builtin-message', text: 'hello', attachments: [], runtimeMode: 'approval-required' });
  assert(engine.requests.some(entry => entry.method === 'session/prompt' && entry.params.sessionId === 'built-in'));
  assert(!engine.requests.some(entry => entry.method === 'hoh/executor/access/set' && entry.params.sessionId === 'built-in'));
  host.stop();
});

test('an unbound session is not treated as a built-in agent', async () => {
  const engine = new FakeEngine({ access: { mode: 'full', selected: { executor: 'claude', model: 'claude-sonnet' } } });
  engine.seed('unbound', 'Unbound', []);
  const original = engine.request.bind(engine);
  engine.request = (method, params) => method === 'hoh/executor/access/get' && params.sessionId === 'unbound'
    ? Promise.resolve({ session: { executor: null, runtimeMode: null } }) : original(method, params);
  const { client, host } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  const snapshot = await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(row => row.id === 'unbound'));
  assert.equal(snapshot.snapshot.threads.find(row => row.id === 'unbound').runtimeMode, 'unknown');
  const sent = await client.request('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'unbound-send', threadId: 'unbound', messageId: 'm', text: 'hello', attachments: [], runtimeMode: 'approval-required' });
  assert.equal(sent._tag, 'Failure');
  assert(!engine.requests.some(entry => entry.method === 'session/prompt' && entry.params.sessionId === 'unbound'));
  host.stop();
});

test('a running turn projects its captured mode rather than the next selected mode', async () => {
  const engine = new FakeEngine();
  Object.assign(engine.seed('mode-running', 'Running', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full' });
  const original = engine.request.bind(engine);
  engine.request = async (method, params) => {
    const result = await original(method, params);
    if (method === 'hoh/executor/access/get' && params.sessionId === 'mode-running')
      return { ...result, session: { ...result.session, runningRuntimeMode: 'confirm', pending: true } };
    return result;
  };
  const { client, host } = connect(engine);
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'mode-running' });
  await stream.until(value => value.kind === 'snapshot');
  engine.emitStatus('mode-running', 'running');
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'approval-required');
  assert.equal(snapshot.projection.thread.runtimeMode, 'approval-required');
  host.stop();
});

test('a delayed running-mode read does not block forced stop', async () => {
  const engine = new FakeEngine();
  Object.assign(engine.seed('delayed-running', 'Running', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full' });
  const original = engine.request.bind(engine);
  let delay = false, release;
  const gate = new Promise(resolve => { release = resolve; });
  engine.request = async (method, params) => {
    const result = await original(method, params);
    if (delay && method === 'hoh/executor/access/get' && params.sessionId === 'delayed-running') {
      await gate;
      return { ...result, session: { ...result.session, runningRuntimeMode: 'confirm', pending: true } };
    }
    return result;
  };
  const { client, host } = connect(engine);
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'delayed-running' });
  await stream.until(value => value.kind === 'snapshot');
  await tick(50);
  delay = true;
  engine.emitStatus('delayed-running', 'running');
  engine.askApproval('delayed-running', 'delay-approval', 'Allow command?');
  await tick(30);
  await client.call('orchestration.dispatchCommand', { type: 'runtime-request.respond', commandId: 'delay-answer', threadId: 'delayed-running', requestId: 'delay-approval', decision: 'accept' });
  assert.deepEqual(engine.decisions, [{ approvalId: 'delay-approval', outcome: 'allowed-once' }]);
  await client.call('orchestration.dispatchCommand', { type: 'thread.stop', commandId: 'delay-stop', threadId: 'delayed-running' });
  assert.deepEqual(await client.call('orchestration.forceStopThread', { threadId: 'delayed-running' }), { outcome: 'killed', sharedWithThreadIds: [] });
  release();
  host.stop();
});

test('Claude acceptEdits launch is projected as auto-accept-edits', async () => {
  const engine = new FakeEngine();
  Object.assign(engine.seed('native-edits', 'Edits', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm', runningRuntimeMode: 'acceptEdits' });
  const { client, host } = connect(engine);
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'native-edits' });
  await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'auto-accept-edits');
  engine.emitEvent('native-edits', 'plugin:external-harness/launch', { runtimeMode: 'acceptEdits' });
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'auto-accept-edits');
  assert.equal(snapshot.projection.thread.runtimeMode, 'auto-accept-edits');
  host.stop();
});

test('the actual running interaction stays separate from the next selected interaction', async () => {
  const engine = new FakeEngine();
  Object.assign(engine.seed('interaction-running', 'Running', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full' });
  const { client, host } = connect(engine);
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'interaction-running' });
  await stream.until(value => value.kind === 'snapshot');
  engine.emitEvent('interaction-running', 'plugin:external-harness/launch', { runtimeMode: 'full', interactionMode: 'default' });
  await client.call('orchestration.dispatchCommand', { type: 'thread.interaction-mode.set', commandId: 'choose-plan-next', threadId: 'interaction-running', interactionMode: 'plan' });
  let projected = (await client.call('orchestration.getThreadProjection', { threadId: 'interaction-running' })).thread;
  assert.equal(projected.runtimeMode, 'full-access');
  assert.equal(projected.interactionMode, 'default', 'the current Full turn must not become Plan');
  assert.equal(host.thread('interaction-running').interactionMode, 'plan', 'the next choice is retained');
  engine.emitEvent('interaction-running', 'plugin:external-harness/launch', { runtimeMode: 'plan', interactionMode: 'plan' });
  await client.call('orchestration.dispatchCommand', { type: 'thread.interaction-mode.set', commandId: 'choose-default-next', threadId: 'interaction-running', interactionMode: 'default' });
  projected = (await client.call('orchestration.getThreadProjection', { threadId: 'interaction-running' })).thread;
  assert.equal(projected.interactionMode, 'plan', 'the current Plan turn must remain Plan');
  assert.equal(host.thread('interaction-running').interactionMode, 'default', 'the next choice is separate');
  host.stop();
});

test('a new turn is unknown until its own launch, rather than borrowing the previous Plan', async () => {
  const engine = new FakeEngine();
  Object.assign(engine.seed('new-turn-mode', 'Turns', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full' });
  const { client, host } = connect(engine);
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'new-turn-mode' });
  await stream.until(value => value.kind === 'snapshot');
  engine.emitEvent('new-turn-mode', 'plugin:external-harness/launch', { runtimeMode: 'plan', interactionMode: 'plan' });
  engine.emitEvent('new-turn-mode', 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  const original = engine.request.bind(engine);
  engine.request = (method, params) => method === 'hoh/executor/access/get' && params.sessionId === 'new-turn-mode' ? new Promise(() => {}) : original(method, params);
  engine.emitEvent('new-turn-mode', 'turn/start', { turn: 2 });
  const pending = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'unknown');
  assert.notEqual(pending.projection.thread.interactionMode, 'plan');
  engine.emitEvent('new-turn-mode', 'plugin:external-harness/launch', { runtimeMode: 'full', interactionMode: 'default' });
  const started = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'full-access');
  assert.equal(started.projection.thread.interactionMode, 'default');
  host.stop();
});

function connectColdHistory(engine, threadId, options) {
  const { title, updatedAt } = engine.store.get(threadId);
  const storage = {
    getItem: key => key === mod.host.STORAGE_KEY
      ? JSON.stringify({ version: 2, sessions: [{ id: threadId, title, createdAt: updatedAt, updatedAt }] })
      : null,
    setItem() {},
  };
  // Cached rows can open while session/list is pending. Keep the background Home scan out of
  // the request count so these tests measure the cold subscription's own retry budget.
  const request = engine.request.bind(engine);
  const pendingList = new Promise(() => {});
  engine.request = (method, params) => method === 'session/list' ? pendingList : request(method, params);
  return connect(engine, { ...options, storage });
}

test('cold history retries temporary bridge failures and deduplicates replayed events and approvals', async () => {
  const id = 'history-overload';
  const engine = new FakeEngine();
  const user = { id: 'first-message', source: { kind: 'user' }, content: [{ type: 'text', text: 'Existing question' }] };
  const session = engine.seed(id, 'Existing chat', [['user/message', user], ['turn/start', { turn: 1 }]]);
  for (let i = session.events.length; i < 1000; i++) engine.persist(id, 'padding', {});
  engine.persist(id, 'assistant/message', { turn: 1, step: 1, message: { id: 'answer', content: [{ type: 'text', text: 'Recovered answer' }] } });

  const originalRequest = engine.request.bind(engine);
  const errors = [
    { code: 'failed', message: 'Помощник занят. Повторите запрос позже' },
    { code: 'failed', message: 'Помощник недоступен' },
  ];
  let firstPages = 0;
  let followingPages = 0;
  engine.request = async (method, params) => {
    if (method !== 'session/events' || params.sessionId !== id) return originalRequest(method, params);
    if (params.afterSeq === undefined) {
      firstPages++;
    } else if (followingPages++ < errors.length) {
      engine.notify('session.event', { sessionId: id, event: session.events[0] });
      engine.askApproval(id, 'approval-during-retry', 'Allow once?');
      throw errors[followingPages - 1];
    }
    return originalRequest(method, params);
  };
  const { client, host } = connectColdHistory(engine, id, { loadRetries: 2, loadRetryMs: 5 });
  try {
    const shell = await client.stream('orchestration.subscribeShell', { requestCompletionMarker: true });
    await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(row => row.id === id));
    const stream = await client.stream('orchestration.subscribeThread', { threadId: id, requestCompletionMarker: true });
    await stream.until(value => value.kind === 'synchronized' || stream.exit !== undefined);
    assert.equal(stream.exit, undefined, JSON.stringify(stream.exit));
    const { projection } = lastSnapshot(stream);
    assert.equal(projection.messages.filter(message => message.role === 'user' && message.text === 'Existing question').length, 1);
    assert.equal(projection.messages.filter(message => message.role === 'assistant' && message.text === 'Recovered answer').length, 1);
    assert.equal(projection.runtimeRequests.filter(request => request.id === 'approval-during-retry').length, 1);
    assert.equal(projection.turnItems.filter(item => item.requestId === 'approval-during-retry').length, 1);
    assert.equal(followingPages, 3);
    assert.equal(firstPages, 3, 'each failed historical read replays an already applied page');
  } finally {
    host.stop();
    client.close();
  }
});

test('cold history stops retrying when the read budget is exhausted', async () => {
  const id = 'history-still-unavailable';
  const engine = new FakeEngine();
  Object.assign(engine.seed(id, 'Existing chat', []), { executor: undefined });
  const originalRequest = engine.request.bind(engine);
  let failures = 0;
  engine.request = async (method, params) => {
    if (method === 'session/events' && params.sessionId === id) {
      if (engine.open.has(id)) failures++;
      throw { code: 'failed', message: 'Помощник недоступен' };
    }
    return originalRequest(method, params);
  };
  const { client, host } = connectColdHistory(engine, id, { loadRetries: 2, loadRetryMs: 10 });
  try {
    const shell = await client.stream('orchestration.subscribeShell', {});
    await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(row => row.id === id));
    const stream = await client.stream('orchestration.subscribeThread', { threadId: id, requestCompletionMarker: true });
    await stream.until(() => stream.exit !== undefined);
    assert.equal(stream.exit._tag, 'Failure');
    assert.equal(failures, 3, 'one attempt and two retries');
    assert(!stream.values.some(value => value.kind === 'synchronized'));
  } finally {
    host.stop();
    client.close();
  }
});

test('a cold conversation publishes an approval even while its access read never returns', async () => {
  const engine = new FakeEngine();
  engine.seed('cold-pending', 'Cold pending', []);
  const original = engine.request.bind(engine);
  engine.request = (method, params) => method === 'hoh/executor/access/get' && params.sessionId === 'cold-pending' ? new Promise(() => {}) : original(method, params);
  const { client, host } = connect(engine, { loadRetries: 0 });
  const shell = await client.stream('orchestration.subscribeShell', {});
  await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(row => row.id === 'cold-pending'));
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'cold-pending' });
  await stream.until(value => value.kind === 'snapshot');
  engine.askApproval('cold-pending', 'cold-pending-approval', 'Allow?');
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.some(item => item.id === 'cold-pending-approval'));
  assert(snapshot.projection.turnItems.some(item => item.type === 'approval_request' && item.requestId === 'cold-pending-approval'));
  await client.call('orchestration.dispatchCommand', { type: 'runtime-request.respond', commandId: 'answer-cold-pending', threadId: 'cold-pending', requestId: 'cold-pending-approval', decision: 'accept' });
  assert.deepEqual(engine.decisions, [{ approvalId: 'cold-pending-approval', outcome: 'allowed-once' }]);
  host.stop();
});

test('launch before a delayed access answer keeps the newer mode and recovers the executor', async () => {
  const engine = new FakeEngine();
  const { client, host } = connect(engine, { loadRetries: 0 });
  await client.call('server.getConfig', {});
  Object.assign(engine.seed('notification-launch', 'Scheduled', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full' });
  const original = engine.request.bind(engine);
  let release;
  let bindingReads = 0;
  const gate = new Promise(resolve => { release = resolve; });
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && params.sessionId === 'notification-launch') {
      if (++bindingReads > 2) throw new Error('retry unavailable');
      const stale = await original(method, params);
      await gate;
      return stale;
    }
    return original(method, params);
  };
  engine.emitStatus('notification-launch', 'running');
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'notification-launch' });
  await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'unknown');
  engine.store.get('notification-launch').runningRuntimeMode = 'confirm';
  engine.emitEvent('notification-launch', 'plugin:external-harness/launch', { runtimeMode: 'confirm', interactionMode: 'default' });
  await tick(10);
  assert.equal(host.thread('notification-launch').bindingKnown, false);
  release();
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.providerInstanceId === 'claudeAgent');
  assert.equal(snapshot.projection.thread.runtimeMode, 'approval-required');
  host.stop();
});

test('a launch retries a failed first binding read', async () => {
  const engine = new FakeEngine();
  const { client, host } = connect(engine, { loadRetries: 0 });
  await client.call('server.getConfig', {});
  Object.assign(engine.seed('retry-binding', 'Scheduled', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm' });
  const original = engine.request.bind(engine);
  let reads = 0;
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && params.sessionId === 'retry-binding' && ++reads === 1) throw new Error('temporary error');
    return original(method, params);
  };
  engine.emitEvent('retry-binding', 'user/message', { id: 'retry-user', source: { kind: 'user' }, content: [{ type: 'text', text: 'hello' }] });
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'retry-binding' });
  await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'unknown');
  assert.equal(reads, 1);
  engine.store.get('retry-binding').runningRuntimeMode = 'confirm';
  engine.emitEvent('retry-binding', 'plugin:external-harness/launch', { runtimeMode: 'confirm', interactionMode: 'default' });
  const known = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.providerInstanceId === 'claudeAgent');
  assert.equal(known.projection.thread.runtimeMode, 'approval-required');
  assert(reads >= 2);
  host.stop();
});

test('a running Claude Plan from access/get remains visible as Plan with Full selected', async () => {
  const engine = new FakeEngine();
  Object.assign(engine.seed('running-plan', 'Plan', [
    ['user/message', { id: 'running-plan-message', source: { kind: 'user' }, content: [{ type: 'text', text: 'Plan this' }] }],
    ['turn/start', { turn: 1 }],
  ]), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full', runningRuntimeMode: 'plan' });
  const { client, host } = connect(engine);
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'running-plan' });
  await stream.until(value => value.kind === 'snapshot');
  engine.notify('session.status', { sessionId: 'running-plan', status: 'running' });
  await tick(30);
  host.flush();
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.interactionMode === 'plan');
  assert.equal(snapshot.projection.thread.runtimeMode, 'approval-required');
  assert.equal(snapshot.projection.thread.interactionMode, 'plan');
  host.stop();
});

test('an approval arriving before any access response is visible and answerable with unknown binding', async () => {
  const engine = new FakeEngine();
  const request = engine.request.bind(engine);
  engine.request = (method, params) => method === 'hoh/executor/access/get' ? Promise.reject(new Error('unavailable')) : request(method, params);
  const { client, host } = connect(engine, { loadRetries: 0 });
  await client.call('server.getConfig', {});
  engine.askApproval('unknown-approval', 'approval-unknown', 'Allow the command?');
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'unknown-approval' });
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.some(item => item.id === 'approval-unknown'));
  assert.equal(snapshot.projection.thread.providerInstanceId, 'unknown');
  assert.equal(snapshot.projection.thread.runtimeMode, 'unknown');
  assert(snapshot.projection.turnItems.some(item => item.type === 'approval_request' && item.requestId === 'approval-unknown'));
  await client.call('orchestration.dispatchCommand', { type: 'runtime-request.respond', commandId: 'unknown-answer', threadId: 'unknown-approval', requestId: 'approval-unknown', decision: 'accept' });
  assert.deepEqual(engine.decisions, [{ approvalId: 'approval-unknown', outcome: 'allowed-once' }]);
  host.stop();
});

test('a stalled session access read does not hide its row or other conversations', async () => {
  const engine = new FakeEngine();
  engine.seed('stalled-access', 'Stalled', []);
  engine.seed('other-access', 'Other', []);
  const request = engine.request.bind(engine);
  engine.request = (method, params) => method === 'hoh/executor/access/get' && params.sessionId === 'stalled-access' ? new Promise(() => {}) : request(method, params);
  const { client, host } = connect(engine, { loadRetries: 0 });
  const lifecycle = await client.stream('subscribeServerLifecycle', {});
  await lifecycle.until(value => value.type === 'welcome' && value.payload.bootstrapStatus === 'complete');
  const shell = await client.stream('orchestration.subscribeShell', {});
  const rows = await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.length === 2);
  assert.equal(rows.snapshot.threads.find(row => row.id === 'stalled-access').runtimeMode, 'unknown');
  engine.askApproval('stalled-access', 'stalled-approval', 'Allow?');
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'stalled-access' });
  await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.some(item => item.id === 'stalled-approval'));
  host.stop();
});

test('a stalled access read cannot prevent other saved Home cards from recovering', async () => {
  const engine = new FakeEngine();
  const completed = id => [
    ['turn/start', { turn: 1 }],
    ['user/message', { id, source: { kind: 'user' }, content: [{ type: 'text', text: id }] }],
    ['assistant/message', { turn: 1, message: { id: `${id}-answer`, content: [{ type: 'text', text: 'done' }] } }],
    ['turn/end', { turn: 1, reason: { kind: 'completed' } }],
  ];
  engine.seed('slow-card', 'Slow', completed('slow'));
  engine.seed('ready-card', 'Ready', completed('ready'));
  const request = engine.request.bind(engine);
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && params.sessionId === 'slow-card') await gate;
    return request(method, params);
  };
  const { host } = connect(engine, { loadRetries: 0, now: () => engine.clock });
  await host.start();
  for (let i = 0; i < 100 && !host.home.entries().some(entry => entry.id === 'ready-card'); i++) await tick(5);
  assert.equal(host.home.entries().find(entry => entry.id === 'ready-card')?.state, 'waiting');
  release();
  for (let i = 0; i < 100 && !host.home.entries().some(entry => entry.id === 'slow-card'); i++) await tick(5);
  assert.equal(host.home.entries().find(entry => entry.id === 'slow-card')?.state, 'waiting');
  host.stop();
});

test('a provisional project conversation does not invent a duplicate scratch project', async () => {
  const engine = new FakeEngine();
  engine.seed('project-pending', 'Project chat', []);
  const request = engine.request.bind(engine);
  engine.request = (method, params) => method === 'hoh/executor/access/get' && params.sessionId === 'project-pending' ? new Promise(() => {}) : request(method, params);
  const saved = JSON.stringify({ version: 1, defaultFolder: '/work/project', projects: [{ id: 'p', title: 'Project', workspaceRoot: '/work/project', createdAt: 10, updatedAt: 10 }], threads: { 'project-pending': 'p' } });
  const storage = { getItem: key => key === mod.host.STORAGE_KEY ? saved : null, setItem() {} };
  const { client, host } = connect(engine, { storage, loadRetries: 0 });
  const shell = await client.stream('orchestration.subscribeShell', {});
  const rows = await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(row => row.id === 'project-pending'));
  assert.deepEqual(rows.snapshot.projects.map(project => project.id), ['p']);
  assert.equal(rows.snapshot.threads.find(row => row.id === 'project-pending').projectId, 'p');
  host.stop();
});

test('stop and approval commands do not wait for history or access reads', async () => {
  const engine = new FakeEngine();
  Object.assign(engine.seed('cold-control', 'Cold', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm' });
  const request = engine.request.bind(engine);
  const never = new Promise(() => {});
  let blocked = false;
  engine.request = (method, params) => blocked && (method === 'session/events' || (method === 'hoh/executor/access/get' && params.sessionId === 'cold-control')) ? never : request(method, params);
  const { client, host } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  await shell.until(value => value.kind === 'snapshot');
  blocked = true;
  engine.askApproval('cold-control', 'cold-approval', 'Allow?');
  await client.call('orchestration.dispatchCommand', { type: 'runtime-request.respond', commandId: 'answer-cold', threadId: 'cold-control', requestId: 'cold-approval', decision: 'accept' });
  await client.call('orchestration.dispatchCommand', { type: 'thread.stop', commandId: 'stop-cold', threadId: 'cold-control' });
  await client.call('orchestration.forceStopThread', { threadId: 'cold-control' });
  assert.deepEqual(engine.decisions, [{ approvalId: 'cold-approval', outcome: 'allowed-once' }]);
  assert.equal(engine.cancelled, 2);
  host.stop();
});

test('each message keeps its own permissions and interaction across overlapping sends', async () => {
  const engine = new FakeEngine();
  Object.assign(engine.seed('send-race', 'Race', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm' });
  const original = engine.request.bind(engine);
  let release, entered;
  const gate = new Promise(resolve => { release = resolve; });
  const started = new Promise(resolve => { entered = resolve; });
  const observed = [];
  engine.request = async (method, params) => {
    if (method === 'session/prompt' && params.sessionId === 'send-race') {
      observed.push({ mode: engine.sessionAccess.get('send-race'), interaction: params.interactionMode });
      if (observed.length === 1) { entered(); await gate; }
    }
    return original(method, params);
  };
  const { client, host } = connect(engine);
  await client.call('orchestration.getThreadProjection', { threadId: 'send-race' });
  const send = (id, runtimeMode, interactionMode) => client.call('orchestration.dispatchCommand', {
    type: 'message.dispatch', commandId: id, threadId: 'send-race', messageId: id,
    text: id, attachments: [], runtimeMode, interactionMode,
  });
  const a = send('a', 'approval-required', 'plan');
  await started;
  const b = send('b', 'full-access', 'default');
  await tick(30);
  assert.equal(engine.sessionAccess.get('send-race'), 'confirm');
  release();
  await Promise.all([a, b]);
  assert.deepEqual(observed, [{ mode: 'confirm', interaction: 'plan' }, { mode: 'full', interaction: 'default' }]);
  host.stop();
});

test('a closed Codex chat uses the Codex default model before resume', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-codex-closed', 'Codex saved chat', []);
  engine.store.get('chat-codex-closed').executor = 'codex';
  const { client } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  const snapshot = await shell.until(value => value.kind === 'snapshot');
  const chat = snapshot.snapshot.threads.find(thread => thread.id === 'chat-codex-closed');
  assert.equal(chat.providerInstanceId, 'codex');
  assert.deepEqual(chat.modelSelection, { instanceId: 'codex', model: 'gpt-6-sol' });
  assert.equal(
    engine.requests.some(request => request.method === 'session/resume'),
    false,
    'the shell model must be correct before opening the saved chat',
  );
});

test('a notification-first conversation reads its exact engine mode before it is used', async () => {
  const engine = new FakeEngine();
  const { client, host } = connect(engine, { loadRetryMs: 10 });
  await client.call('server.getConfig', {});
  const session = engine.seed('notification-first', 'Scheduled', []);
  Object.assign(session, { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full' });
  const request = engine.request.bind(engine);
  let release;
  const pending = new Promise(resolve => { release = resolve; });
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && params.sessionId === 'notification-first') await pending;
    return request(method, params);
  };
  engine.emitStatus('notification-first', 'running');
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'notification-first' });
  const shell = await client.stream('orchestration.subscribeShell', {});
  const rows = await shell.until(value => value.kind === 'snapshot');
  assert(rows.snapshot.threads.some(row => row.id === 'notification-first'), 'the running conversation remains reachable while access is pending');
  assert.deepEqual(await client.call('orchestration.forceStopThread', { threadId: 'notification-first' }), { outcome: 'killed', sharedWithThreadIds: [] });
  await tick(20);
  release();
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'full-access');
  assert.equal(snapshot.projection.thread.runtimeMode, 'full-access');
  session.runtimeMode = 'auto';
  engine.emitStatus('notification-first', 'idle');
  engine.setStatus('ready');
  const refreshed = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'auto');
  assert.equal(refreshed.projection.thread.runtimeMode, 'auto', 'later inventory reconciles a notification-created row');
  host.stop();
});

test('a notification-first row takes its running mode from the engine', async () => {
  const engine = new FakeEngine();
  const { client, host } = connect(engine);
  await client.call('server.getConfig', {});
  Object.assign(engine.seed('first-running-mode', 'Running', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm', runningRuntimeMode: 'full' });
  engine.emitFrame('first-running-mode', {});
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'first-running-mode' });
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'full-access');
  assert.equal(snapshot.projection.thread.runtimeMode, 'full-access');
  host.stop();
});

test('a late inventory response cannot replace a newer notification mode', async () => {
  const engine = new FakeEngine();
  const { client, host } = connect(engine);
  await client.call('server.getConfig', {});
  Object.assign(engine.seed('racing-list', 'Race', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full' });
  const original = engine.request.bind(engine);
  let release, started;
  const gate = new Promise(resolve => { release = resolve; });
  const entered = new Promise(resolve => { started = resolve; });
  let first = true;
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && params.sessionId === 'racing-list' && first) {
      first = false; started(); await gate;
      return { session: { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm' } };
    }
    return original(method, params);
  };
  engine.setStatus('ready');
  await entered;
  engine.emitStatus('racing-list', 'running');
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'racing-list' });
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'full-access');
  release(); await tick(50);
  assert.equal(lastSnapshot(stream).projection.thread.runtimeMode, snapshot.projection.thread.runtimeMode);
  host.stop();
});

test('a failed newer inventory read does not discard a pending exact notification mode', async () => {
  const engine = new FakeEngine();
  const { client, host } = connect(engine);
  await client.call('server.getConfig', {});
  Object.assign(engine.seed('failed-list', 'Race', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full' });
  const original = engine.request.bind(engine);
  let release, calls = 0;
  const gate = new Promise(resolve => { release = resolve; });
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && params.sessionId === 'failed-list') {
      calls++;
      if (calls === 1) await gate;
      else throw new Error('inventory unavailable');
    }
    return original(method, params);
  };
  engine.emitFrame('failed-list', {});
  engine.setStatus('ready');
  await tick(30);
  assert(calls >= 2);
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'failed-list' });
  release();
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'full-access');
  assert.equal(snapshot.projection.thread.runtimeMode, 'full-access');
  host.stop();
});

test('a failed cold access read does not treat cached executor or mode as current', async () => {
  const engine = new FakeEngine({ access: { mode: 'full', selected: { executor: 'codex', model: 'gpt-6-sol' } } });
  Object.assign(engine.seed('known-claude', 'Known', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm' });
  const entries = new Map([[mod.host.STORAGE_KEY, JSON.stringify({ version: 2, sessions: [
    { id: 'known-claude', title: 'Known', createdAt: 100, updatedAt: 200, executor: 'claude', model: 'claude-sonnet', mode: 'confirm', projectId: 'hoh-project' },
  ] })]]);
  const storage = { getItem: key => entries.get(key) ?? null, setItem: (key, value) => entries.set(key, value) };
  const request = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && params.sessionId === 'known-claude') throw new Error('unavailable');
    return request(method, params);
  };
  const { client, host } = connect(engine, { storage });
  const shell = await client.stream('orchestration.subscribeShell', {});
  const snapshot = await shell.until(value => value.kind === 'snapshot');
  const row = snapshot.snapshot.threads.find(thread => thread.id === 'known-claude');
  assert.equal(row.providerInstanceId, 'unknown');
  assert.equal(row.runtimeMode, 'unknown');
  const sent = await client.request('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'known-message', threadId: 'known-claude', messageId: 'known-message', text: 'hello', attachments: [], runtimeMode: 'approval-required' });
  assert.equal(sent._tag, 'Failure');
  assert.equal(engine.requests.some(entry => entry.method === 'session/prompt'), false);
  host.stop();
});

test('delayed discovery resolves closed Codex models before shell materialization', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-codex-default', 'Catalog default', []);
  engine.store.get('chat-codex-default').executor = 'codex';
  engine.seed('chat-codex-explicit', 'Persisted model', []);
  Object.assign(engine.store.get('chat-codex-explicit'), { executor: 'codex', model: 'gpt-persisted' });
  const request = engine.request.bind(engine);
  let releaseDiscovery;
  const discoveryMayFinish = new Promise(resolve => { releaseDiscovery = resolve; });
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/discover' && params.executor === 'codex') await discoveryMayFinish;
    return request(method, params);
  };

  const { client } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  await tick(0);
  assert.equal(
    shell.values.some(value => value.kind === 'snapshot'),
    false,
    'a Codex row without a persisted model must wait for the provider catalog before publication',
  );
  releaseDiscovery();
  const snapshot = await shell.until(value => value.kind === 'snapshot');
  const row = id => snapshot.snapshot.threads.find(thread => thread.id === id);
  assert.deepEqual(row('chat-codex-default').modelSelection, { instanceId: 'codex', model: 'gpt-6-sol' });
  assert.deepEqual(row('chat-codex-explicit').modelSelection, { instanceId: 'codex', model: 'gpt-persisted' });
  assert.equal(engine.requests.some(entry => entry.method === 'session/resume'), false);
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
  assert.deepEqual(engine.requests.find(r => r.method === 'session/prompt').params, { sessionId: 'thread-1', contentBlocks: [{ type: 'text', text: 'Посчитай файлы' }], interactionMode: 'default' });
  const user = launched.projection.turnItems.find(item => item.type === 'user_message');
  assert.equal(user.messageId, 'm-t3-1'); assert.equal(user.text, 'Посчитай файлы');
  assert.equal(launched.projection.runs[0].status, 'running');
  assert.equal(launched.projection.thread.modelSelection.model, 'claude-opus');

  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'thread-1', requestCompletionMarker: true });
  await thread.until(value => value.kind === 'synchronized');
  await tick(5); // the engine's echo of the message and turn/start
  assert.equal(itemsOf(lastSnapshot(thread)).filter(item => item.type === 'user_message').length, 1, 'the echo is not shown twice');

  engine.emitFrame('thread-1', { type: 'start', attemptId: 'reason-1', revision: 1, turn: 1, step: 1 });
  engine.emitFrame('thread-1', { type: 'chunk', attemptId: 'reason-1', revision: 1, index: 0, time: 1, chunk: { type: 'reasoning-delta', text: '' } });
  const thinkingStarted = await thread.until(value => value.kind === 'snapshot' && itemsOf(value).some(item => item.type === 'reasoning' && item.text === ''));
  const emptyReasoning = itemsOf(thinkingStarted).find(item => item.type === 'reasoning');
  assert.equal(emptyReasoning.streaming, true); assert.equal(emptyReasoning.status, 'running');
  engine.emitFrame('thread-1', { type: 'chunk', attemptId: 'reason-1', revision: 1, index: 1, time: 2, chunk: { type: 'reasoning-delta', text: 'Обдумываю ' } });
  const thinking = await thread.until(value => value.kind === 'snapshot' && itemsOf(value).some(item => item.type === 'reasoning' && item.text === 'Обдумываю '));
  const liveReasoning = itemsOf(thinking).find(item => item.type === 'reasoning');
  assert.equal(liveReasoning.streaming, true); assert.equal(liveReasoning.status, 'running');

  // A text attempt may overlap the reasoning attempt; neither start retires the other.
  engine.emitFrame('thread-1', { type: 'start', attemptId: 'att-1', revision: 1, turn: 1, step: 1 });
  engine.emitFrame('thread-1', { type: 'chunk', attemptId: 'att-1', revision: 1, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Сейчас ' } });
  engine.emitFrame('thread-1', { type: 'chunk', attemptId: 'att-1', revision: 1, index: 1, time: 2, chunk: { type: 'text-delta', text: 'посмотрю.' } });
  const streaming = await thread.until(value => value.kind === 'snapshot' && itemsOf(value).some(item => item.type === 'assistant_message' && item.text === 'Сейчас посмотрю.'));
  const live = itemsOf(streaming).find(item => item.type === 'assistant_message');
  assert.equal(live.streaming, true); assert.equal(live.status, 'running');
  assert.equal(itemsOf(streaming).find(item => item.type === 'reasoning').text, 'Обдумываю ');
  engine.emitFrame('thread-1', { type: 'end', attemptId: 'reason-1', revision: 1, index: 2, outcome: 'committed' });
  engine.emitFrame('thread-1', { type: 'end', attemptId: 'att-1', revision: 1, index: 2, outcome: 'completed' });
  engine.emitEvent('thread-1', 'assistant/message', { turn: 1, step: 1, message: { id: 'a1', content: [{ type: 'reasoning', text: 'Проверил.' }, { type: 'text', text: 'Сейчас посмотрю.' }] } });

  engine.emitEvent('thread-1', 'tool/call', { turn: 1, callId: 'c1', name: 'Bash', arguments: JSON.stringify({ command: 'ls | wc -l' }) });
  const running = await thread.until(value => value.kind === 'snapshot' && itemsOf(value).some(item => item.type === 'command_execution'));
  const step = itemsOf(running).find(item => item.type === 'command_execution');
  assert.equal(step.input, 'ls | wc -l'); assert.equal(step.status, 'running');
  engine.emitEvent('thread-1', 'tool/result', { turn: 1, message: { toolCallId: 'c1', content: [{ type: 'text', text: '7\n' }] } });
  const done = await thread.until(value => value.kind === 'snapshot' && itemsOf(value).some(item => item.type === 'command_execution' && item.status === 'completed'));
  assert.equal(itemsOf(done).find(item => item.type === 'command_execution').output, '7\n');

  // The reverse start order is also independent: a later reasoning start cannot reset text.
  engine.emitFrame('thread-1', { type: 'start', attemptId: 'att-2', revision: 1, turn: 1, step: 2 });
  engine.emitFrame('thread-1', { type: 'chunk', attemptId: 'att-2', revision: 1, index: 0, time: 3, chunk: { type: 'text-delta', text: 'Файлов: ' } });
  engine.emitFrame('thread-1', { type: 'start', attemptId: 'reason-2', revision: 1, turn: 1, step: 2 });
  engine.emitFrame('thread-1', { type: 'chunk', attemptId: 'reason-2', revision: 1, index: 0, time: 4, chunk: { type: 'reasoning-delta', text: 'Сверяю.' } });
  engine.emitFrame('thread-1', { type: 'chunk', attemptId: 'att-2', revision: 1, index: 1, time: 5, chunk: { type: 'text-delta', text: '7.' } });
  const reverse = await thread.until(value => value.kind === 'snapshot'
    && itemsOf(value).some(item => item.type === 'reasoning' && item.text === 'Сверяю.')
    && itemsOf(value).some(item => item.type === 'assistant_message' && item.text === 'Файлов: 7.'));
  assert.equal(itemsOf(reverse).find(item => item.type === 'assistant_message' && item.text === 'Файлов: 7.').streaming, true);
  engine.emitEvent('thread-1', 'assistant/message', { turn: 1, step: 2, message: { id: 'a2', content: [{ type: 'reasoning', text: 'Сверено.' }, { type: 'text', text: 'Файлов: 7.' }] } });
  engine.emitEvent('thread-1', 'turn/end', { reason: { kind: 'completed' } });
  const ended = await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'completed');
  const answers = itemsOf(ended).filter(item => item.type === 'assistant_message');
  assert.deepEqual(answers.map(item => [item.text, item.streaming, item.status]), [['Сейчас посмотрю.', false, 'completed'], ['Файлов: 7.', false, 'completed']]);
  const reasoning = itemsOf(ended).filter(item => item.type === 'reasoning');
  assert.deepEqual(reasoning.map(item => [item.text, item.streaming, item.status]), [['Проверил.', false, 'completed'], ['Сверено.', false, 'completed']]);
  assert.deepEqual(ended.projection.visibleTurnItems.map(row => row.item.type), ['user_message', 'reasoning', 'assistant_message', 'command_execution', 'assistant_message', 'reasoning']);
  assert(ended.snapshotSequence > 0);

  const row = await shell.until(value => value.kind === 'thread.updated' && value.thread.id === 'thread-1' && value.thread.status === 'completed');
  assert.equal(row.thread.latestVisibleMessage.text, 'Файлов: 7.');
  assert.equal(row.thread.title, 'Hello');
});

test('Plan mode follows the T3 composer command and ExitPlanMode becomes its proposed-plan artifact', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  await client.call('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'p0', threadId: 'thread-plan', projectId: 'hoh-project', title: 'Plan',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'plan',
  });
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'thread-plan' });
  await thread.until(value => value.kind === 'snapshot');
  await client.call('orchestration.dispatchCommand', {
    type: 'message.dispatch', commandId: 'p1', threadId: 'thread-plan', messageId: 'm-plan', text: 'Составь план', attachments: [],
    createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' },
  });
  assert.equal(engine.requests.find(r => r.method === 'session/prompt').params.interactionMode, 'plan');
  engine.emitEvent('thread-plan', 'tool/call', { turn: 1, callId: 'exit-plan', name: 'ExitPlanMode', arguments: { plan: '# План\n\n1. Проверить код\n2. Внести правку' } });
  const planned = await thread.until(value => value.kind === 'snapshot' && value.projection.plans.length === 1 && value.projection.thread.interactionMode === 'plan');
  assert.equal(planned.projection.thread.interactionMode, 'plan');
  assert.equal(planned.projection.plans[0].status, 'active');
  assert.equal(planned.projection.turnItems.find(item => item.type === 'proposed_plan').markdown, '# План\n\n1. Проверить код\n2. Внести правку');
  engine.emitEvent('thread-plan', 'tool/call', { turn: 1, callId: 'exit-plan-2', name: 'ExitPlanMode', arguments: { plan: '# Новый план' } });
  const replanned = await thread.until(value => value.kind === 'snapshot' && value.projection.plans.length === 2);
  assert.deepEqual(replanned.projection.plans.map(plan => plan.status), ['superseded', 'active']);
  await client.call('orchestration.dispatchCommand', { type: 'thread.interaction-mode.set', commandId: 'p2', threadId: 'thread-plan', interactionMode: 'default' });
  const stillPlan = await client.call('orchestration.getThreadProjection', { threadId: 'thread-plan' });
  assert.equal(stillPlan.thread.interactionMode, 'plan');
  engine.emitEvent('thread-plan', 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  const ordinary = await thread.until(value => value.kind === 'snapshot' && value.projection.thread.interactionMode === 'default');
  assert.equal(ordinary.projection.thread.interactionMode, 'default');
});

test('unknown interaction modes are rejected instead of becoming an ordinary prompt', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  await assert.rejects(() => client.call('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'bad-create', threadId: 'thread-bad-mode', projectId: 'hoh-project', title: 'Bad',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'future-mode',
  }), /Unknown interaction mode/);
  assert.equal(engine.requests.some(request => request.method === 'session/prompt'), false);

  await client.call('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'good-create', threadId: 'thread-known-mode', projectId: 'hoh-project', title: 'Known',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'plan',
  });
  await assert.rejects(() => client.call('orchestration.dispatchCommand', {
    type: 'thread.interaction-mode.set', commandId: 'bad-set', threadId: 'thread-known-mode', interactionMode: 'future-mode',
  }), /Unknown interaction mode/);
  const projection = await client.call('orchestration.getThreadProjection', { threadId: 'thread-known-mode' });
  assert.equal(projection.thread.interactionMode, 'plan');

  const accessSets = engine.requests.filter(request => request.method === 'hoh/executor/access/set').length;
  const prompts = engine.requests.filter(request => request.method === 'session/prompt').length;
  await assert.rejects(() => client.call('orchestration.launchThread', {
    commandId: 'bad-resume', threadId: 'thread-known-mode', projectId: 'hoh-project',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'full-access', interactionMode: 'future-mode',
    initialMessage: { messageId: 'bad-message', text: 'must not run', attachments: [] },
  }), /Unknown interaction mode/);
  assert.equal(engine.requests.filter(request => request.method === 'hoh/executor/access/set').length, accessSets);
  assert.equal(engine.requests.filter(request => request.method === 'session/prompt').length, prompts);
  const unchanged = await client.call('orchestration.getThreadProjection', { threadId: 'thread-known-mode' });
  assert.equal(unchanged.thread.interactionMode, 'plan');
  assert.equal(unchanged.thread.runtimeMode, 'approval-required');
});

test('Claude stream usage reaches T3 providerTurns with its exact context fill', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  await client.call('orchestration.launchThread', {
    commandId: 'u0', threadId: 'thread-usage', projectId: 'hoh-project', title: 'Usage',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default',
    initialMessage: { messageId: 'm-usage', text: 'Покажи контекст', attachments: [] },
  });
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'thread-usage' });
  await stream.until(value => value.kind === 'snapshot');
  engine.emitEvent('thread-usage', 'plugin:hoh/context-usage', {
    usage: { inputTokens: 200, cacheCreationInputTokens: 40, cacheReadInputTokens: 60, outputTokens: 9 },
  });
  const live = await stream.until(value => value.kind === 'snapshot' && value.projection.providerTurns.length === 1);
  assert.equal(live.projection.providerTurns[0].status, 'running');
  assert.equal(live.projection.providerTurns[0].tokenUsage.usedTokens, 309);
  assert.equal(live.projection.providerTurns[0].tokenUsage.maxTokens, undefined);
  engine.emitEvent('thread-usage', 'turn/usage', {
    usage: { inputTokens: 200, cacheCreationInputTokens: 40, cacheReadInputTokens: 60, outputTokens: 9, maxTokens: 200_000 },
  });
  engine.emitEvent('thread-usage', 'turn/end', {
    reason: { kind: 'completed' },
    usage: { inputTokens: 200, cacheCreationInputTokens: 40, cacheReadInputTokens: 60, outputTokens: 9, maxTokens: 200_000 },
  });
  const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.providerTurns[0]?.status === 'completed' && value.projection.providerTurns[0]?.tokenUsage.maxTokens === 200_000);
  const usage = snapshot.projection.providerTurns[0].tokenUsage;
  assert.deepEqual(usage, { usedTokens: 309, maxTokens: 200_000, inputTokens: 300, cachedInputTokens: 60, outputTokens: 9, reasoningOutputTokens: 0, updatedAt: usage.updatedAt });
  assert.equal((usage.usedTokens / usage.maxTokens) * 100, 0.1545);
});

test('cold history restores usage from both plugin and legacy events', async () => {
  for (const eventType of ['plugin:hoh/context-usage', 'turn/usage']) {
    const id = 'cold-' + eventType;
    const engine = new FakeEngine();
    Object.assign(engine.seed(id, 'Usage', [
      ['turn/start', { turn: 1 }],
      ['user/message', { id: 'user', source: { kind: 'user' }, content: [{ type: 'text', text: 'hello' }] }],
      [eventType, { turn: 1, usage: { inputTokens: 200, cacheCreationInputTokens: 40, cacheReadInputTokens: 60, outputTokens: 9, maxTokens: 200_000 } }],
      ['turn/end', { turn: 1, reason: { kind: 'completed' } }],
    ]), { executor: 'claude', runtimeMode: 'confirm' });
    const { client, host } = connectColdHistory(engine, id, {});
    try {
      const shell = await client.stream('orchestration.subscribeShell', {});
      await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(row => row.id === id));
      const stream = await client.stream('orchestration.subscribeThread', { threadId: id });
      const snapshot = await stream.until(value => value.kind === 'snapshot' && value.projection.providerTurns.length === 1);
      assert.equal(snapshot.projection.providerTurns[0].tokenUsage.usedTokens, 309);
      assert.equal(snapshot.projection.providerTurns[0].tokenUsage.maxTokens, 200_000);
    } finally {
      host.stop();
      client.close();
    }
  }
});

test('the first message gets a Claude title; failure keeps its beginning and a late result cannot overwrite a manual rename', async () => {
  const launch = (client, id, text) => client.call('orchestration.launchThread', {
    commandId: 'title-' + id, threadId: id, projectId: 'hoh-project',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default',
    initialMessage: { messageId: 'm-' + id, text, attachments: [] },
  });

  const success = new FakeEngine(); const successRequest = success.request.bind(success);
  success.request = (method, params) => {
    if (method === 'hoh/executor/title/generate') { success.requests.push({ method, params }); return Promise.resolve({ title: 'Context Window Indicator' }); }
    return successRequest(method, params);
  };
  const titleData = new Map();
  const titleStorage = { getItem: key => titleData.get(key) ?? null, setItem: (key, value) => titleData.set(key, value) };
  const successful = connect(success, { storage: titleStorage }); await launch(successful.client, 'title-ok', 'Покажи заполнение контекста'); await tick(5);
  assert.equal((await successful.client.call('orchestration.getThreadProjection', { threadId: 'title-ok' })).thread.title, 'Context Window Indicator');
  const titleCall = success.requests.find(request => request.method === 'hoh/executor/title/generate');
  assert.equal(titleCall.params.executor, 'claude'); assert.match(titleCall.params.prompt, /3-8 words/);
  successful.host.stop();
  const reopenedTitle = connect(new FakeEngine({ store: success.store }), { storage: titleStorage });
  await reopenedTitle.host.start(); await tick(10);
  assert.equal((await reopenedTitle.client.call('orchestration.getThreadProjection', { threadId: 'title-ok' })).thread.title, 'Context Window Indicator');
  reopenedTitle.host.stop();

  const failed = new FakeEngine(); const failedRequest = failed.request.bind(failed);
  failed.request = (method, params) => method === 'hoh/executor/title/generate' ? Promise.reject(new Error('offline')) : failedRequest(method, params);
  const failure = connect(failed); await launch(failure.client, 'title-fail', 'Сохрани начало сообщения при ошибке'); await tick(5);
  assert.equal((await failure.client.call('orchestration.getThreadProjection', { threadId: 'title-fail' })).thread.title, 'Сохрани начало сообщения при ошибке');

  let finishTitle; const pending = new Promise(resolve => { finishTitle = resolve; });
  const stale = new FakeEngine(); const staleRequest = stale.request.bind(stale);
  stale.request = (method, params) => method === 'hoh/executor/title/generate' ? pending : staleRequest(method, params);
  const values = new Map();
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
  const guarded = connect(stale, { storage }); await launch(guarded.client, 'title-stale', 'Первое имя');
  await guarded.client.call('orchestration.dispatchCommand', { type: 'thread.metadata.update', commandId: 'rename', threadId: 'title-stale', title: 'Моё название' });
  finishTitle({ title: 'Late Generated Title' }); await tick(5);
  assert.equal((await guarded.client.call('orchestration.getThreadProjection', { threadId: 'title-stale' })).thread.title, 'Моё название');

  const namedEngine = new FakeEngine();
  const named = connect(namedEngine, { storage });
  await named.client.call('orchestration.dispatchCommand', { type: 'thread.create', commandId: 'create-named', threadId: 'named-first', projectId: 'hoh-project', title: 'Draft', createdBy: 'user', creationSource: 'web', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default', branch: null, worktreePath: null });
  await named.client.call('orchestration.dispatchCommand', { type: 'thread.metadata.update', commandId: 'name-first', threadId: 'named-first', title: 'Моё имя' });
  await named.client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'send-named', threadId: 'named-first', messageId: 'named-message', text: 'hello', attachments: [], runtimeMode: 'approval-required' });
  await tick(5);
  assert.equal((await named.client.call('orchestration.getThreadProjection', { threadId: 'named-first' })).thread.title, 'Моё имя');
  assert(!namedEngine.requests.some(request => request.method === 'hoh/executor/title/generate'));
  named.host.stop();
});

test('a follow-up message goes to the same chat; Stop is the engine\'s cancel and the run ends interrupted', async () => {
  const engine = new FakeEngine();
  const { client, host } = connect(engine);
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
  engine.emitEvent('thread-2', 'turn/end', { reason: { kind: 'aborted', reason: { kind: 'user' } } });
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'interrupted');
  const stopped = host.home.entries().find(entry => entry.id === 'thread-2');
  assert.deepEqual(
    [stopped.state, stopped.interrupted, stopped.stopped, stopped.canComplete],
    ['waiting', undefined, true, true],
    'the person\'s explicit Stop stays visible without becoming a crash card',
  );
  assert.deepEqual(await client.call('orchestration.forceStopThread', { threadId: 'thread-2' }), { outcome: 'killed', sharedWithThreadIds: [] });
  assert.equal(engine.cancelled, 2);
});

test('a durable user Stop survives restart and later turns for Claude and Codex', async () => {
  for (const executor of ['claude', 'codex']) {
    const store = new Map();
    const first = new FakeEngine({ store });
    const id = `stopped-${executor}`;
    first.seed(id, `Stopped ${executor}`, [
      ['turn/start', { turn: 1 }],
      ['user/message', { id: `${id}-u1`, source: { kind: 'user' }, content: [{ type: 'text', text: 'first' }] }],
      ['tool/call', { turn: 1, callId: `${id}-tool`, name: 'Read', arguments: { file_path: 'note.txt' } }],
      ['turn/end', { turn: 1, reason: { kind: 'aborted', reason: { kind: 'user' } } }],
      ['turn/start', { turn: 2 }],
      ['user/message', { id: `${id}-u2`, source: { kind: 'user' }, content: [{ type: 'text', text: 'second' }] }],
      ['assistant/message', { turn: 2, step: 1, message: { id: `${id}-a2`, content: [{ type: 'text', text: 'done' }] } }],
      ['turn/end', { turn: 2, reason: { kind: 'completed' } }],
    ]);
    Object.assign(store.get(id), {
      executor, model: executor === 'codex' ? 'gpt-6-sol' : 'claude-sonnet', runtimeMode: 'confirm',
    });

    for (let restart = 0; restart < 2; restart++) {
      const engine = restart === 0 ? first : new FakeEngine({ store });
      const { client, host } = connect(engine);
      const projection = await client.call('orchestration.getThreadProjection', { threadId: id });
      assert.deepEqual(projection.runs.map(run => run.status), ['interrupted', 'completed'], `${executor} restart ${restart}`);
      const stops = projection.turnItems.filter(item => item.type === 'run_interrupt_result');
      assert.deepEqual(stops.map(item => [item.runId, item.message]), [[`run-msg-${id}-u1`, 'Stopped by user']], `${executor} restart ${restart}`);
      host.stop();
    }
  }
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
  engine.emitEvent('chat-p', 'approval/asked', { id: 'appr-1', toolName: 'bash', callId: 'call-1' });
  engine.emitEvent('chat-p', 'tool/call', { turn: 1, step: 2, callId: 'decoy', name: 'write', arguments: JSON.stringify({ file_path: '/home/person/wrong.txt' }) });
  engine.askApproval('chat-p', 'appr-1', 'Разрешить точную команду rm -rf tmp?');
  const asked = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.length === 1);
  const request = asked.projection.runtimeRequests[0];
  assert.deepEqual([request.id, request.kind, request.status, request.responseCapability.type], ['appr-1', 'command', 'pending', 'live']);
  const item = itemsOf(asked).find(entry => entry.type === 'approval_request');
  assert.deepEqual([item.requestId, item.requestKind, item.prompt], ['appr-1', 'command', 'Разрешить точную команду rm -rf tmp?']);
  assert.deepEqual(item.options.map(option => option.label), ['Decline', 'Approve']);
  assert.doesNotMatch(item.prompt, /wrong\.txt/, 'a later running call cannot replace the signed request');
  assert.equal(asked.projection.runs[0].status, 'waiting');
  await client.call('orchestration.dispatchCommand', { type: 'runtime-request.respond', commandId: 'c1', threadId: 'chat-p', requestId: 'appr-1', decision: 'accept' });
  assert.deepEqual(engine.decisions, [{ approvalId: 'appr-1', outcome: 'allowed-once' }]);
  const answered = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests[0]?.status === 'resolved');
  assert.equal(answered.projection.runtimeRequests[0].decision, 'accept');
  // The engine's own resolution afterwards changes nothing.
  engine.notify('approval.resolved', { approvalId: 'appr-1', outcome: 'allowed-once' });
  // No call id still keeps the engine's id/text; toolName supplies only the T3 request kind.
  engine.emitEvent('chat-p', 'tool/call', { turn: 1, step: 2, callId: 'call-2', name: 'write', arguments: JSON.stringify({ file_path: '/home/person/notes.txt', content: 'x' }) });
  engine.emitEvent('chat-p', 'approval/asked', { id: 'appr-2', toolName: 'write' });
  engine.askApproval('chat-p', 'appr-2', 'Изменить notes.txt?');
  const write = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.length === 2);
  assert.equal(write.projection.runtimeRequests[1].kind, 'file-change');
  assert.equal(itemsOf(write).find(entry => entry.requestId === 'appr-2').prompt, 'Изменить notes.txt?');
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

test('access mode: T3\'s exact switch is scoped to the thread; the default setting stays where new threads start', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-m', 'M', []);
  const { client } = connect(engine);
  const config = await client.stream('subscribeServerConfig', {});
  await config.until(value => value.type === 'snapshot');
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'c1', threadId: 'chat-m', runtimeMode: 'approval-required' });
  assert.deepEqual(engine.requests.filter(r => r.method === 'hoh/executor/access/set').map(r => r.params), [{ executor: 'claude', sessionId: 'chat-m', mode: 'confirm', runtimeMode: 'confirm' }]);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'chat-m' })).thread.runtimeMode, 'approval-required');
  // T3 shows the mode chip under the composer of a started thread while its mode differs from this setting.
  assert.equal((await client.call('server.getSettings', {})).defaultRuntimeMode, 'full-access');
  // T3's in-between modes keep their exact Codex meaning (the legacy field stays confirm).
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'c2', threadId: 'chat-m', runtimeMode: 'auto-accept-edits' });
  assert.equal(engine.sessionAccess.get('chat-m'), 'auto-accept-edits');
});

test('Full is only the fresh-chat default; a version-1 browser cache cannot widen a recordless legacy conversation', async () => {
  const items = new Map([[mod.host.STORAGE_KEY, JSON.stringify({ version: 1, defaultMode: 'confirm' })]]);
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const engine = new FakeEngine();
  engine.seed('legacy-chat', 'Legacy', []);
  Object.assign(engine.store.get('legacy-chat'), { executor: 'claude', model: 'claude-sonnet' });
  const { client } = connect(engine, { storage });
  const shell = await client.stream('orchestration.subscribeShell', {});
  const snapshot = await shell.until(value => value.kind === 'snapshot');
  assert.equal(snapshot.snapshot.threads.find(thread => thread.id === 'legacy-chat').runtimeMode, 'approval-required');
  assert.equal((await client.call('server.getSettings', {})).defaultRuntimeMode, 'full-access');

  await client.call('orchestration.launchThread', {
    commandId: 'fresh', threadId: 'fresh-chat', projectId: 'hoh-project', title: 'Fresh',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, interactionMode: 'default', workspaceStrategy: { type: 'root' },
  });
  assert.equal(engine.sessionAccess.get('fresh-chat'), 'full');
  assert.equal(JSON.parse(items.get(mod.host.STORAGE_KEY)).version, 2);
});

test('the thread always shows the engine running mode while a different choice is pending', async () => {
  const engine = new FakeEngine();
  engine.seed('running-mode', 'Running mode', []);
  Object.assign(engine.store.get('running-mode'), {
    executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm', runningRuntimeMode: 'full',
  });
  const { client } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  const snapshot = await shell.until(value => value.kind === 'snapshot');
  assert.equal(snapshot.snapshot.threads.find(thread => thread.id === 'running-mode').runtimeMode, 'full-access');

  await client.call('orchestration.dispatchCommand', {
    type: 'thread.runtime-mode.set', commandId: 'pending-confirm', threadId: 'running-mode', runtimeMode: 'approval-required',
  });
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'running-mode' })).thread.runtimeMode, 'full-access');

  await client.call('orchestration.dispatchCommand', {
    type: 'thread.runtime-mode.set', commandId: 'return-full', threadId: 'running-mode', runtimeMode: 'full-access',
  });
  assert.equal(engine.sessionAccess.get('running-mode'), 'full', 'returning to the running mode replaces the pending choice');
  await client.call('orchestration.dispatchCommand', {
    type: 'thread.runtime-mode.set', commandId: 'pending-confirm-again', threadId: 'running-mode', runtimeMode: 'approval-required',
  });
  delete engine.store.get('running-mode').runningRuntimeMode;
  engine.emitStatus('running-mode', 'idle');
  await tick(5);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'running-mode' })).thread.runtimeMode, 'approval-required');
});

test('each queued message captures its own access choice even when it matches the running turn', async () => {
  const engine = new FakeEngine();
  engine.seed('queue-modes', 'Queued', []);
  Object.assign(engine.store.get('queue-modes'), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm', runningRuntimeMode: 'confirm' });
  const observed = [];
  const request = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'session/prompt') observed.push(engine.sessionAccess.get(params.sessionId));
    const result = await request(method, params);
    if (method === 'session/prompt') engine.store.get('queue-modes').runningRuntimeMode = 'confirm';
    return result;
  };
  const { client } = connect(engine);
  await (await client.stream('orchestration.subscribeShell', {})).until(value => value.kind === 'snapshot');
  for (const [id, runtimeMode] of [['B', 'full-access'], ['C', 'approval-required']]) {
    await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: id, threadId: 'queue-modes', messageId: id, text: id, attachments: [], runtimeMode, dispatchMode: { type: 'queue_after_active' } });
  }
  assert.deepEqual(observed, ['full', 'confirm']);
});

test('the launch event publishes B full while C supervised is queued and defeats stale access replies', async () => {
  const engine = new FakeEngine();
  engine.seed('queue-current', 'Queued', []);
  Object.assign(engine.store.get('queue-current'), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm', runningRuntimeMode: 'confirm' });
  const { client } = connect(engine);
  await (await client.stream('orchestration.subscribeShell', {})).until(value => value.kind === 'snapshot');
  const original = engine.request.bind(engine);
  engine.request = async (method, params) => {
    const result = await original(method, params);
    if (method === 'session/prompt') engine.store.get('queue-current').runningRuntimeMode = 'confirm';
    return result;
  };
  for (const [id, runtimeMode] of [['B', 'full-access'], ['C', 'approval-required']]) {
    await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: id, threadId: 'queue-current', messageId: id, text: id, attachments: [], runtimeMode, dispatchMode: { type: 'queue_after_active' } });
  }
  assert.equal(engine.sessionAccess.get('queue-current'), 'confirm');
  let release;
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && params.sessionId === 'queue-current' && !release) {
      const stale = await original(method, params);
      return await new Promise(resolve => { release = () => resolve(stale); });
    }
    return original(method, params);
  };
  engine.emitEvent('queue-current', 'turn/start', { turn: 2 });
  await tick(5);
  engine.store.get('queue-current').runningRuntimeMode = 'full';
  engine.emitEvent('queue-current', 'plugin:external-harness/launch', { runtimeMode: 'full' });
  release();
  await tick(5);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'queue-current' })).thread.runtimeMode, 'full-access');
  assert.equal(engine.store.get('queue-current').runtimeMode, 'confirm', 'C remains Supervised');
});

test('Codex: app-server models and effort are offered; executor and exact mode apply before the first message', async () => {
  const engine = new FakeEngine();
  const { client, host } = connect(engine);
  const config = await client.call('server.getConfig', {});
  const codex = config.providers.find(provider => provider.instanceId === 'codex');
  assert.equal(codex.driver, 'codex');
  assert.equal(codex.showInteractionModeToggle, false);
  assert.deepEqual(codex.models.map(model => model.slug), ['gpt-6-sol']);
  assert.deepEqual(codex.models[0].capabilities.optionDescriptors[0].options.map(option => [option.id, !!option.isDefault]), [
    ['low', false], ['medium', true], ['high', false],
  ]);

  const launched = await client.call('orchestration.launchThread', {
    commandId: 'cx-launch', threadId: 'cx-chat', projectId: 'hoh-project', title: 'Codex task',
    modelSelection: { instanceId: 'codex', model: 'gpt-6-sol', options: [{ id: 'reasoningEffort', value: 'high' }] },
    runtimeMode: 'auto', interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'cx-message', text: 'work', attachments: [] },
  });
  assert.equal(launched.projection.thread.providerInstanceId, 'codex');
  assert.equal(launched.projection.thread.runtimeMode, 'unknown');
  await tick(20);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'cx-chat' })).thread.runtimeMode, 'auto');
  const selected = engine.requests.find(request => request.method === 'hoh/executor/select' && request.params.executor === 'codex');
  const access = engine.requests.find(request => request.method === 'hoh/executor/access/set' && request.params.sessionId === 'cx-chat');
  const prompt = engine.requests.find(request => request.method === 'session/prompt' && request.params.sessionId === 'cx-chat');
  assert.deepEqual(selected.params, { executor: 'codex', model: 'gpt-6-sol' });
  assert.deepEqual(access.params, { executor: 'codex', sessionId: 'cx-chat', mode: 'confirm', runtimeMode: 'auto' });
  assert.equal(prompt.params.effort, 'high');
  assert(engine.requests.indexOf(access) < engine.requests.indexOf(prompt));
  assert.equal(engine.requests.some(request => request.method === 'hoh/executor/title/generate'), false, 'Codex text never goes to Claude');
  await assert.rejects(client.call('orchestration.dispatchCommand', {
    type: 'thread.interaction-mode.set', commandId: 'codex-plan', threadId: 'cx-chat', interactionMode: 'plan',
  }), /coming soon/i);
  assert.equal(host.home.entries()[0].providerInstanceId, 'codex');

  const unsupported = new FakeEngine({ codexDiscovery: {
    executor: 'codex', installed: true, version: '0.200.0', versionStatus: 'unsupported',
    auth: 'authenticated', detail: null, models: [{ id: 'future-model', isDefault: true }],
  } });
  const unsupportedConfig = await connect(unsupported).client.call('server.getConfig', {});
  const unsupportedCodex = unsupportedConfig.providers.find(provider => provider.instanceId === 'codex');
  assert.equal(unsupportedCodex.status, 'error');
  assert.match(unsupportedCodex.message, /not supported/);
});

test('a restored global Codex selection remains the default provider and model', async () => {
  const engine = new FakeEngine({
    access: { mode: 'confirm', selected: { executor: 'codex', model: 'gpt-6-sol' } },
  });
  const { client } = connect(engine);
  const config = await client.call('server.getConfig', {});
  assert.deepEqual(config.settings.defaultModelSelection, { instanceId: 'codex', model: 'gpt-6-sol' });
});

test('Codex: two conversations keep independent modes instead of racing through a provider global', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  const create = (threadId, runtimeMode) => client.call('orchestration.launchThread', {
    commandId: 'create-' + threadId, threadId, projectId: 'hoh-project', title: threadId,
    modelSelection: { instanceId: 'codex', model: 'gpt-6-sol' }, runtimeMode,
    interactionMode: 'default', workspaceStrategy: { type: 'root' },
  });
  const [a, b] = await Promise.all([create('cx-a', 'auto-accept-edits'), create('cx-b', 'full-access')]);
  assert.deepEqual([a.projection.thread.runtimeMode, b.projection.thread.runtimeMode], ['auto-accept-edits', 'full-access']);
  assert.deepEqual([...engine.sessionAccess.entries()].sort(), [['cx-a', 'auto-accept-edits'], ['cx-b', 'full']]);
});

test('concurrent Claude and Codex launches atomically bind their requested executor, model, and mode', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  const launch = (threadId, instanceId, model, runtimeMode) => client.call('orchestration.launchThread', {
    commandId: 'parallel-' + threadId, threadId, projectId: 'hoh-project', title: threadId,
    modelSelection: { instanceId, model }, runtimeMode,
    interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'message-' + threadId, text: 'hello', attachments: [] },
  });
  const [claude, codex] = await Promise.all([
    launch('parallel-claude', 'claudeAgent', 'claude-opus', 'approval-required'),
    launch('parallel-codex', 'codex', 'gpt-6-sol', 'auto-accept-edits'),
  ]);
  assert.deepEqual(
    [claude.projection.thread.providerInstanceId, codex.projection.thread.providerInstanceId],
    ['claudeAgent', 'codex'],
  );
  assert.deepEqual(
    ['parallel-claude', 'parallel-codex'].map(id => {
      const session = engine.store.get(id);
      return [id, session.executor, session.model, session.runtimeMode];
    }),
    [
      ['parallel-claude', 'claude', 'claude-opus', 'confirm'],
      ['parallel-codex', 'codex', 'gpt-6-sol', 'auto-accept-edits'],
    ],
  );
  for (const id of ['parallel-claude', 'parallel-codex']) {
    const access = engine.requests.find(request => request.method === 'hoh/executor/access/set' && request.params.sessionId === id);
    const prompt = engine.requests.find(request => request.method === 'session/prompt' && request.params.sessionId === id);
    assert(engine.requests.indexOf(access) < engine.requests.indexOf(prompt));
  }
});

test('new-session binding excludes every overlapping global model-selection path', async () => {
  for (const mutation of ['message.dispatch', 'thread.model-selection.set', 'server.updateSettings']) {
    const engine = new FakeEngine();
    engine.seed('existing', 'Existing', []);
    Object.assign(engine.store.get('existing'), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm' });
    const { client } = connect(engine);
    await client.call('orchestration.getThreadProjection', { threadId: 'existing' });

    const originalRequest = engine.request.bind(engine);
    let selectedCodex; let releaseCreate;
    const codexSelected = new Promise(resolve => { selectedCodex = resolve; });
    const createMayContinue = new Promise(resolve => { releaseCreate = resolve; });
    engine.request = async (method, params) => {
      const result = await originalRequest(method, params);
      if (method === 'hoh/executor/select' && params.executor === 'codex') {
        selectedCodex();
        await createMayContinue;
      }
      return result;
    };

    const threadId = 'created-' + mutation;
    const launching = client.call('orchestration.launchThread', {
      commandId: 'launch-' + mutation, threadId, projectId: 'hoh-project', title: threadId,
      modelSelection: { instanceId: 'codex', model: 'gpt-6-sol' }, runtimeMode: 'auto-accept-edits',
      interactionMode: 'default', workspaceStrategy: { type: 'root' },
      initialMessage: { messageId: 'first-' + mutation, text: 'first send', attachments: [] },
    });
    await codexSelected;
    let changing;
    if (mutation === 'message.dispatch') {
      changing = client.call('orchestration.dispatchCommand', {
        type: mutation, commandId: 'mutate-message', threadId: 'existing', messageId: 'existing-message',
        text: 'existing send', attachments: [], modelSelection: { instanceId: 'claudeAgent', model: 'claude-opus' },
      });
    } else if (mutation === 'thread.model-selection.set') {
      changing = client.call('orchestration.dispatchCommand', {
        type: mutation, commandId: 'mutate-model', threadId: 'existing',
        modelSelection: { instanceId: 'claudeAgent', model: 'claude-opus' },
      });
    } else {
      changing = client.call('server.updateSettings', {
        patch: { defaultModelSelection: { instanceId: 'claudeAgent', model: 'claude-opus' } },
      });
    }
    await tick(0);
    releaseCreate();
    const [launched] = await Promise.all([launching, changing]);
    assert.equal(launched.projection.thread.providerInstanceId, 'codex', mutation);
    assert.deepEqual(
      [engine.store.get(threadId).executor, engine.store.get(threadId).model, engine.store.get(threadId).runtimeMode],
      ['codex', 'gpt-6-sol', 'auto-accept-edits'],
      mutation,
    );
    assert(
      engine.requests.some(request => request.method === 'session/prompt' && request.params.sessionId === threadId),
      `${mutation}: the first send did not reach its bound session`,
    );
    const methods = engine.requests.map(request => [request.method, request.params.executor, request.params.sessionId]);
    const create = methods.findIndex(([method, , session]) => method === 'session/working-directory/get' && session === threadId);
    const laterClaudeSelection = methods.findIndex(([method, executor], index) => method === 'hoh/executor/select' && executor === 'claude' && index > create);
    assert(create >= 0 && laterClaudeSelection > create, `${mutation}: global selection entered the create critical section`);
  }
});

// Access-read fail-closed coverage from the Claude line applies to both executor providers.

test('access mode stays unknown until ready, sends no mode or prompt, then rereads every ready', async () => {
  const engine = new FakeEngine({ access: { mode: 'full', selected: { executor: 'claude', model: 'claude-sonnet' } } });
  engine.seed('chat-late', 'Late', []);
  let ready = false;
  const request = engine.request.bind(engine);
  engine.request = (method, params) => method === 'hoh/executor/access/get' && !ready
    ? Promise.reject(new Error('engine is starting'))
    : request(method, params);
  const { client } = connect(engine, { loadRetryMs: 60_000, loadRetries: 1 });
  const config = await client.stream('subscribeServerConfig', {});
  const unknown = await config.until(value => value.type === 'snapshot');
  assert.deepEqual(unknown.config.providers[0].supportedRuntimeModes, []);
  assert.equal('defaultRuntimeMode' in unknown.config.settings, false);
  const shell = await client.stream('orchestration.subscribeShell', {});
  const rows = await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(row => row.id === 'chat-late'));
  assert.equal(rows.snapshot.threads.find(row => row.id === 'chat-late').runtimeMode, 'unknown');

  const failed = await client.request('orchestration.launchThread', {
    commandId: 'unknown-mode', threadId: 'unknown-mode', projectId: 'hoh-project', title: 'Blocked',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required',
    interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'm-unknown', text: 'must not run', attachments: [] },
  });
  assert.equal(failed._tag, 'Failure');
  assert.equal(engine.requests.some(entry => entry.method === 'hoh/executor/access/set' || entry.method === 'session/prompt'), false);

  ready = true;
  engine.setStatus('ready');
  const known = await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 4);
  assert.equal(known.config.providers[0].supportedRuntimeModes.at(-1), 'full-access');
  await tick(5);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'chat-late' })).thread.runtimeMode, 'full-access');

  engine.access = { ...engine.access, mode: 'auto' };
  engine.setStatus('ready');
  await tick(5);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'chat-late' })).thread.runtimeMode, 'auto');
});

test('engine loss immediately hides modes from config and refreshProviders until ready rereads access', async () => {
  const engine = new FakeEngine({ access: { mode: 'full', selected: { executor: 'claude', model: 'claude-sonnet' } } });
  const { client } = connect(engine);
  const config = await client.stream('subscribeServerConfig', {});
  const known = await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 4);
  assert.equal(known.config.settings.defaultRuntimeMode, 'full-access');

  const afterKnown = config.values.length;
  engine.setStatus('restarting');
  const lost = await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 0, afterKnown);
  assert.equal('defaultRuntimeMode' in lost.config.settings, false);
  assert.deepEqual((await client.call('server.refreshProviders', {})).providers[0].supportedRuntimeModes, []);
  const workspaceRefresh = await client.call('server.refreshProviders', { instanceId: 'claudeAgent', cwd: '/home/person/Documents/HoH' });
  assert.deepEqual(workspaceRefresh.providers[0].supportedRuntimeModes, []);
  assert.equal(workspaceRefresh.providers[0].workspaceSnapshots.length, 1);

  engine.access = { ...engine.access, mode: 'auto' };
  const beforeReady = config.values.length;
  engine.setStatus('ready');
  await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 4, beforeReady);
  assert.deepEqual((await client.call('server.refreshProviders', {})).providers[0].supportedRuntimeModes,
    ['approval-required', 'auto-accept-edits', 'auto', 'full-access']);
});

test('after a restart the chat comes back from the engine\'s history; an unfinished turn waits on Home as interrupted', async () => {
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
  const { client, host } = connect(engine);
  const projection = await client.call('orchestration.getThreadProjection', { threadId: 'chat-r' });
  assert.equal(projection.thread.modelSelection.model, 'claude-opus', 'the chat keeps the model it was bound to');
  const resumeAt = engine.requests.findIndex(r => r.method === 'session/resume');
  assert(resumeAt >= 0 && engine.requests.findIndex((r, index) => index > resumeAt && r.method === 'session/events') > resumeAt, 'opened chat is resumed before its full read');
  assert.deepEqual(projection.runs.map(run => run.status), ['completed', 'interrupted']);
  assert.deepEqual(projection.turnItems.map(item => [item.type, item.status]), [
    ['user_message', 'completed'], ['assistant_message', 'completed'], ['user_message', 'completed'], ['dynamic_tool', 'interrupted'], ['system_notice', 'completed'],
  ]);
  assert.equal(projection.turnItems.at(-1).message, 'Interrupted');
  assert.equal(projection.thread.title, 'Restart me');
  const interrupted = host.home.entries().find(entry => entry.id === 'chat-r');
  assert.deepEqual(
    [interrupted.state, interrupted.interrupted, interrupted.canComplete, interrupted.turnOrdinal],
    ['waiting', true, true, 2],
  );
  await host.home.complete('chat-r');
  assert.equal(host.home.entries().find(entry => entry.id === 'chat-r').state, 'done');
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-r' });
  const snapshot = await thread.until(value => value.kind === 'snapshot');
  assert.equal(snapshot.projection.messages.length, 3);
  // The chat continues after the restart.
  await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c1', threadId: 'chat-r', messageId: 'm-r', text: 'Продолжим', attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' } });
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs.length === 3);
});

test('engine lost mid-turn: the running turn ends as stopped', async () => {
  const items = new Map();
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const store = new Map();
  const engine = new FakeEngine({ store });
  engine.seed('chat-l', 'L', []);
  const { client, host } = connect(engine, { storage });
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-l' });
  await thread.until(value => value.kind === 'snapshot');
  engine.emitEvent('chat-l', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'x' }] });
  engine.emitEvent('chat-l', 'turn/start', {});
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'running');
  engine.setStatus('restarting');
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'interrupted');
  host.flush();
  let interrupted = host.home.entries().find(entry => entry.id === 'chat-l');
  assert.deepEqual([interrupted.state, interrupted.interrupted, interrupted.canComplete], ['waiting', true, true]);
  assert.deepEqual(JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['chat-l'], { turnOrdinal: 1, state: 'waiting', interrupted: true });

  // A late terminal event from the dying engine cannot erase the exact turn already placed in the person's queue.
  engine.emitEvent('chat-l', 'turn/end', { turn: 1, reason: { kind: 'aborted' } });
  host.flush();
  interrupted = host.home.entries().find(entry => entry.id === 'chat-l');
  assert.deepEqual([interrupted.state, interrupted.interrupted, interrupted.canComplete], ['waiting', true, true]);
  await host.home.complete('chat-l');
  assert.deepEqual(JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['chat-l'], { turnOrdinal: 1, state: 'done', interrupted: true });

  // Replay now contains durable `aborted`, so only the persisted Home cause can keep the human decision visible.
  const afterRestart = connect(new FakeEngine({ store }), { storage });
  await afterRestart.host.start();
  const done = afterRestart.host.home.entries().find(entry => entry.id === 'chat-l');
  assert.deepEqual([done.state, done.interrupted, done.canComplete], ['done', true, undefined]);
});

test('a crash placeholder is reconciled when the same turn resumes and completes', async () => {
  const items = new Map();
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const engine = new FakeEngine();
  engine.seed('chat-recovered', 'Recovered', []);
  const { client, host } = connect(engine, { storage, throttleMs: 600000 });
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-recovered' });
  await thread.until(value => value.kind === 'snapshot');
  engine.emitEvent('chat-recovered', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'recover' }] });
  engine.emitEvent('chat-recovered', 'turn/start', { turn: 1 });
  engine.setStatus('restarting');
  host.flush();
  assert.deepEqual(
    [host.home.entries().find(entry => entry.id === 'chat-recovered').state, JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['chat-recovered'].interrupted],
    ['waiting', true],
  );
  assert(host.thread('chat-recovered').projection().turnItems.some(item => item.type === 'system_notice' && item.message === 'Interrupted'));

  engine.emitStatus('chat-recovered', 'running');
  host.flush();
  assert.equal(host.home.entries().find(entry => entry.id === 'chat-recovered').state, 'working');
  assert(!host.thread('chat-recovered').projection().turnItems.some(item => item.type === 'system_notice' && item.message === 'Interrupted'));
  assert.equal(JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['chat-recovered'], undefined);
  engine.emitEvent('chat-recovered', 'assistant/message', { turn: 1, message: { id: 'a1', content: [{ type: 'text', text: 'finished after recovery' }] } });
  engine.emitEvent('chat-recovered', 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  host.flush();
  const completed = host.home.entries().find(entry => entry.id === 'chat-recovered');
  assert.deepEqual([completed.state, completed.interrupted, completed.canComplete], ['waiting', undefined, true]);
});

test('a late completed event directly reconciles an inferred engine loss', async () => {
  const items = new Map();
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const store = new Map();
  const engine = new FakeEngine({ store });
  engine.seed('chat-late-completed', 'Late completed', []);
  const { client, host } = connect(engine, { storage, throttleMs: 600000 });
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-late-completed' });
  await thread.until(value => value.kind === 'snapshot');
  engine.emitEvent('chat-late-completed', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'finish late' }] });
  engine.emitEvent('chat-late-completed', 'turn/start', { turn: 1 });
  engine.setStatus('restarting');
  host.flush();
  assert.equal(host.home.entries().find(entry => entry.id === 'chat-late-completed').interrupted, true);

  // Some engines deliver the durable terminal event without first reporting that the same turn resumed.
  engine.emitEvent('chat-late-completed', 'assistant/message', { turn: 1, message: { id: 'a1', content: [{ type: 'text', text: 'finished after all' }] } });
  engine.emitEvent('chat-late-completed', 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  host.flush();
  const completed = host.home.entries().find(entry => entry.id === 'chat-late-completed');
  assert.deepEqual([completed.state, completed.interrupted, completed.canComplete], ['waiting', undefined, true]);
  assert.deepEqual(JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['chat-late-completed'], { turnOrdinal: 1, state: 'waiting' });

  const afterRestart = connect(new FakeEngine({ store }), { storage });
  await afterRestart.host.start();
  const restored = afterRestart.host.home.entries().find(entry => entry.id === 'chat-late-completed');
  assert.deepEqual([restored.state, restored.interrupted, restored.canComplete], ['waiting', undefined, true]);
});

test('an explicit Stop after recovery remains a stopped waiting card across immediate restart', async () => {
  const items = new Map();
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const engine = new FakeEngine();
  engine.seed('chat-stop-recovered', 'Stop recovered', []);
  const { client, host } = connect(engine, { storage, throttleMs: 600000 });
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-stop-recovered' });
  await thread.until(value => value.kind === 'snapshot');
  engine.emitEvent('chat-stop-recovered', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'recover then stop' }] });
  engine.emitEvent('chat-stop-recovered', 'turn/start', { turn: 1 });
  engine.setStatus('restarting');
  host.flush();
  assert.equal(JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['chat-stop-recovered'].interrupted, true);

  let releaseCancel;
  const cancelGate = new Promise(resolve => { releaseCancel = resolve; });
  const request = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'session/cancel') await cancelGate;
    return request(method, params);
  };

  // Do not flush the recovery before Stop: cancel itself must clear even a not-yet-reconciled crash disposition.
  // Restart before the cancel RPC answers, which is the window-close race that used to resurrect Interrupted.
  engine.emitStatus('chat-stop-recovered', 'running');
  const stopping = client.call('orchestration.dispatchCommand', { type: 'thread.stop', commandId: 'stop-recovered', threadId: 'chat-stop-recovered' });
  await tick(5);
  assert.deepEqual(JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['chat-stop-recovered'], { turnOrdinal: 1, state: 'waiting', stopped: true });

  host.stop();
  const restarted = connect(new FakeEngine({ store: engine.store }), { storage });
  await restarted.host.start();
  const restored = restarted.host.home.entries().find(entry => entry.id === 'chat-stop-recovered');
  assert.deepEqual([restored.state, restored.interrupted, restored.stopped, restored.canComplete], ['waiting', undefined, true, true]);
  releaseCancel();
  await stopping;
  assert.deepEqual(JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['chat-stop-recovered'], { turnOrdinal: 1, state: 'waiting', stopped: true });
  restarted.host.stop();
});

test('a rejected Stop rolls back the durable stopped intent while the original run stays live', async () => {
  const items = new Map();
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const engine = new FakeEngine();
  engine.seed('chat-stop-refused', 'Stop refused', []);
  const request = engine.request.bind(engine);
  engine.request = (method, params) => method === 'session/cancel'
    ? Promise.reject(new Error('cancel refused'))
    : request(method, params);
  const { client, host } = connect(engine, { storage, throttleMs: 600000 });
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-stop-refused' });
  await thread.until(value => value.kind === 'snapshot');
  engine.emitEvent('chat-stop-refused', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'keep running' }] });
  engine.emitEvent('chat-stop-refused', 'turn/start', { turn: 1 });
  host.flush();

  await assert.rejects(
    client.call('orchestration.dispatchCommand', { type: 'thread.stop', commandId: 'stop-refused', threadId: 'chat-stop-refused' }),
    /cancel refused/,
  );
  assert.equal(host.home.entries().find(entry => entry.id === 'chat-stop-refused').state, 'working');
  assert.equal(JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['chat-stop-refused'], undefined);
  host.stop();
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
  assert.equal(engine.sessionAccess.get('thread-f'), 'full');
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
  const accessWrites = engine.requests.filter(r => r.method === 'hoh/executor/access/set').map(r => r.params);
  assert(accessWrites.length >= 1);
  assert(accessWrites.every(params => JSON.stringify(params) === JSON.stringify({ executor: 'claude', sessionId: 'thread-w', mode: 'full', runtimeMode: 'full' })));
  assert(order.indexOf('hoh/executor/access/set') < order.indexOf('session/prompt'), 'the engine has the mode before Claude starts');
  assert.equal(launched.projection.thread.runtimeMode, 'unknown');
  await tick(20);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'thread-w' })).thread.runtimeMode, 'full-access');
  // A started thread: T3 sends the switch right before the next message (ChatView `persistThreadSettingsForNextTurn`).
  engine.requests.length = 0;
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'c1', threadId: 'thread-w', runtimeMode: 'approval-required' });
  await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c2', threadId: 'thread-w', messageId: 'm-2', text: 'again', attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' } });
  assert.deepEqual(engine.requests.filter(r => r.method === 'hoh/executor/access/set' || r.method === 'session/prompt').map(r => [r.method, r.params.mode ?? r.params.contentBlocks?.[0]?.text]),
    [['hoh/executor/access/set', 'confirm'], ['session/prompt', 'again']]);
  assert.equal(engine.sessionAccess.get('thread-w'), 'confirm');
  // A thread created with the default mode starts in it.
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'c3', threadId: 'thread-w', runtimeMode: 'full-access' });
  await client.call('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'c4', threadId: 'thread-n', projectId: 'hoh-project', title: 'N', createdBy: 'user', creationSource: 'web',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default', branch: null, worktreePath: null,
  });
  assert.equal(engine.sessionAccess.get('thread-n'), 'confirm');
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
  const signedOut = connect(new FakeEngine({ discovery: { executor: 'claude', installed: true, version: '2.1.329', auth: 'unauthenticated', models: [] } })).client;
  assert.equal((await signedOut.call('server.getConfig', {})).providers[0].message, 'Claude Code is not authenticated. Run `claude auth login` and try again.');
  const timedOut = connect(new FakeEngine({ discovery: { executor: 'claude', installed: true, version: null, auth: 'unknown', models: [], failure: { kind: 'timeout', operation: 'version', timeoutSeconds: 20 } } })).client;
  assert.equal((await timedOut.call('server.getConfig', {})).providers[0].message, 'Claude Agent CLI health check timed out after 20 seconds while running `claude --version`.');
  const initializeTimedOut = connect(new FakeEngine({ discovery: { executor: 'claude', installed: true, version: '2.1.329', auth: 'unknown', models: [], failure: { kind: 'timeout', operation: 'initialize', timeoutSeconds: 45 } } })).client;
  assert.equal((await initializeTimedOut.call('server.getConfig', {})).providers[0].message, 'Claude Agent CLI health check timed out after 45 seconds while waiting for `initialize`.');
  const exited = connect(new FakeEngine({ discovery: { executor: 'claude', installed: true, version: null, auth: 'unknown', models: [], failure: { kind: 'exit', operation: 'version', exitCode: 7 } } })).client;
  assert.equal((await exited.call('server.getConfig', {})).providers[0].message, 'Claude Agent CLI health check failed: `claude --version` exited with code 7.');
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
  for (const id of ['w1', 'w2', 'p1', 'd1', 'd2']) Object.assign(engine.seed(id, id, []), { executor: 'claude', model: 'claude-sonnet' });
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
  assert.deepEqual([by.w1.state, by.w1.steps, by.w2.state, by.p1.state, by.d1.state, by.d2.state], ['working', { done: 1, total: 2 }, 'working', 'waiting', 'waiting', 'waiting']);
  assert.deepEqual([by.d1.canComplete, by.d2.canComplete], [true, true]);
  assert.deepEqual(by.p1.request, { id: 'appr-h', kind: 'approval', requestKind: 'command', text: 'ignored' });
  assert(by.d1.doneAt > 0 && by.w1.doneAt === null);
  await host.home.complete('d1');
  assert.deepEqual([host.home.entries().find(entry => entry.id === 'd1').state, host.home.entries().find(entry => entry.id === 'd2').state], ['done', 'waiting']);
  // The card's buttons are the chat's own answer.
  let changes = 0; host.home.subscribe(() => { changes++; });
  await host.home.answer('p1', 'appr-h', 'accept');
  assert.deepEqual(engine.decisions, [{ approvalId: 'appr-h', outcome: 'allowed-once' }]);
  assert(changes > 0);
  assert.equal(host.home.entries().find(entry => entry.id === 'p1').request, undefined);
  // A new chat from the home screen's input.
  const id = await host.home.start('Plan the week');
  assert.equal(host.home.entries().find(entry => entry.id === id).state, 'working');
  assert.equal(host.home.projectTitle('hoh-project'), 'Chats');
});

test('home turn lifecycle: Claude reply waits for the person, Mark done survives replay, the next human turn waits again, and old waiting turns are restored', async () => {
  const items = new Map();
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const store = new Map();
  const engine = new FakeEngine({ store });
  const first = connect(engine, { storage, now: () => engine.clock });
  await first.client.call('orchestration.launchThread', {
    commandId: 'c-live', threadId: 'live-home', projectId: 'hoh-project', title: 'Live home',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default',
    workspaceStrategy: { type: 'root' }, initialMessage: { messageId: 'local-message-1', text: 'first', attachments: [] },
  });
  await tick(5);
  engine.emitEvent('live-home', 'assistant/message', { turn: 1, step: 1, message: { id: 'answer-1', content: [{ type: 'text', text: 'ready' }] } });
  engine.emitEvent('live-home', 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  first.host.flush();
  const waiting = first.host.home.entries().find(entry => entry.id === 'live-home');
  assert.deepEqual([waiting.state, waiting.turnOrdinal, waiting.canComplete], ['waiting', 1, true]);

  // Answering directly from Waiting starts a new turn and removes the old human disposition without Mark done.
  engine.counter = 1;
  await first.client.call('orchestration.dispatchCommand', {
    type: 'message.dispatch', commandId: 'c-next', threadId: 'live-home', messageId: 'local-message-2', text: 'next', attachments: [],
    createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' },
  });
  assert.equal(first.host.home.entries().find(entry => entry.id === 'live-home').state, 'working');
  assert.equal(JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['live-home'], undefined);
  await tick(5);
  engine.emitEvent('live-home', 'assistant/message', { turn: 2, step: 1, message: { id: 'answer-2', content: [{ type: 'text', text: 'ready again' }] } });
  engine.emitEvent('live-home', 'turn/end', { turn: 2, reason: { kind: 'completed' } });
  first.host.flush();
  assert.deepEqual(
    [first.host.home.entries().find(entry => entry.id === 'live-home').state, JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['live-home']],
    ['waiting', { turnOrdinal: 2, state: 'waiting' }],
  );

  await first.host.home.complete('live-home');
  assert.equal(first.host.home.entries().find(entry => entry.id === 'live-home').state, 'done');
  const saved = JSON.parse(items.get(mod.host.STORAGE_KEY));
  assert.deepEqual(saved.homeDispositions['live-home'], { turnOrdinal: 2, state: 'done' });

  // Replay uses the engine's message id instead of T3's local id, but ordinal 1 still identifies this exact turn.
  const second = new FakeEngine({ store });
  second.clock = engine.clock;
  const again = connect(second, { storage, now: () => second.clock });
  await again.host.start();
  const projection = await again.client.call('orchestration.getThreadProjection', { threadId: 'live-home' });
  const replayed = again.host.home.entries().find(entry => entry.id === 'live-home');
  assert.notEqual(projection.runs[0].id, 'run-local-message-1');
  assert.deepEqual([replayed.state, replayed.turnOrdinal], ['done', 2]);

  second.counter = 2;
  await again.client.call('orchestration.dispatchCommand', {
    type: 'message.dispatch', commandId: 'c-third', threadId: 'live-home', messageId: 'local-message-3', text: 'one more', attachments: [],
    createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' },
  });
  assert.equal(again.host.home.entries().find(entry => entry.id === 'live-home').state, 'working');
  await tick(5);
  second.emitEvent('live-home', 'assistant/message', { turn: 3, step: 1, message: { id: 'answer-3', content: [{ type: 'text', text: 'ready once more' }] } });
  second.emitEvent('live-home', 'turn/end', { turn: 3, reason: { kind: 'completed' } });
  again.host.flush();
  assert.deepEqual(
    [again.host.home.entries().find(entry => entry.id === 'live-home').state, JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['live-home']],
    ['waiting', { turnOrdinal: 3, state: 'waiting' }],
  );

  // The ordinary "today" peek no longer includes this chat, but a persisted human wait must still load it.
  const later = new FakeEngine({ store });
  later.clock = second.clock + 2 * 24 * 60 * 60 * 1000;
  const restored = connect(later, { storage, now: () => later.clock });
  await restored.host.start();
  await tick(5);
  restored.host.flush();
  const oldWaiting = restored.host.home.entries().find(entry => entry.id === 'live-home');
  assert.deepEqual([oldWaiting.state, oldWaiting.turnOrdinal, oldWaiting.canComplete], ['waiting', 3, true]);
});

test('home disposition migration classifies a completed reply older than today even without prior metadata', async () => {
  const items = new Map([[mod.host.STORAGE_KEY, JSON.stringify({ version: 1 })]]);
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const engine = new FakeEngine();
  engine.seed('before-upgrade', 'Before upgrade', [
    ['turn/start', { turn: 1 }],
    ['user/message', { id: 'old-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'old request' }] }],
    ['assistant/message', { turn: 1, message: { id: 'old-a', content: [{ type: 'text', text: 'old answer' }] } }],
    ['turn/end', { turn: 1, reason: { kind: 'completed' } }],
  ]);
  engine.clock += 2 * 24 * 60 * 60 * 1000;
  const { host } = connect(engine, { storage, now: () => engine.clock });
  await host.start();
  await tick(5); // Home history classification is intentionally outside the T3 startup barrier.
  host.flush();
  const migrated = host.home.entries().find(entry => entry.id === 'before-upgrade');
  assert.deepEqual([migrated.state, migrated.turnOrdinal, migrated.canComplete], ['waiting', 1, true]);
  let saved = JSON.parse(items.get(mod.host.STORAGE_KEY));
  assert.equal(saved.homeObservedAt['before-upgrade'] > 0, true);
  assert.deepEqual(saved.homeDispositions['before-upgrade'], { turnOrdinal: 1, state: 'waiting' });

  // Once classified and marked Done, a newer completion written while this window is away is still discovered by timestamp.
  await host.home.complete('before-upgrade');
  const away = new FakeEngine({ store: engine.store });
  away.clock = engine.clock + 2 * 24 * 60 * 60 * 1000;
  away.emitEvent('before-upgrade', 'user/message', { id: 'new-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'new request' }] });
  away.emitEvent('before-upgrade', 'turn/start', { turn: 2 });
  away.emitEvent('before-upgrade', 'assistant/message', { turn: 2, message: { id: 'new-a', content: [{ type: 'text', text: 'new answer' }] } });
  away.emitEvent('before-upgrade', 'turn/end', { turn: 2, reason: { kind: 'completed' } });
  const reopened = connect(away, { storage, now: () => away.clock });
  await reopened.host.start();
  await tick(5);
  reopened.host.flush();
  const changedWhileAway = reopened.host.home.entries().find(entry => entry.id === 'before-upgrade');
  assert.deepEqual([changedWhileAway.state, changedWhileAway.turnOrdinal, changedWhileAway.canComplete], ['waiting', 2, true]);
  saved = JSON.parse(items.get(mod.host.STORAGE_KEY));
  assert.deepEqual(saved.homeDispositions['before-upgrade'], { turnOrdinal: 2, state: 'waiting' });
});

test('a rejected human send does not clear the completed answer that still waits for the person', async () => {
  const items = new Map();
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const engine = new FakeEngine();
  const { client, host } = connect(engine, { storage, now: () => engine.clock });
  await client.call('orchestration.launchThread', {
    commandId: 'c-fail-live', threadId: 'failed-human-reply', projectId: 'hoh-project', title: 'Failed human reply',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default',
    workspaceStrategy: { type: 'root' }, initialMessage: { messageId: 'first-local', text: 'first', attachments: [] },
  });
  await tick(5);
  engine.emitEvent('failed-human-reply', 'assistant/message', { turn: 1, message: { id: 'first-answer', content: [{ type: 'text', text: 'ready' }] } });
  engine.emitEvent('failed-human-reply', 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  host.flush();
  assert.equal(host.home.entries().find(entry => entry.id === 'failed-human-reply').state, 'waiting');

  const request = engine.request.bind(engine);
  engine.request = (method, params) => method === 'session/prompt' ? Promise.reject(new Error('transport down')) : request(method, params);
  await assert.rejects(client.call('orchestration.dispatchCommand', {
    type: 'message.dispatch', commandId: 'c-failed-answer', threadId: 'failed-human-reply', messageId: 'failed-local', text: 'my answer', attachments: [],
    createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' },
  }));
  host.flush();
  const stillWaiting = host.home.entries().find(entry => entry.id === 'failed-human-reply');
  assert.deepEqual([stillWaiting.state, stillWaiting.turnOrdinal, stillWaiting.canComplete], ['waiting', 1, true]);
  assert.deepEqual(JSON.parse(items.get(mod.host.STORAGE_KEY)).homeDispositions['failed-human-reply'], { turnOrdinal: 1, state: 'waiting' });
  await host.home.complete('failed-human-reply');
  assert.equal(host.home.entries().find(entry => entry.id === 'failed-human-reply').state, 'done');
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
  engine.emitEvent('chat-h', 'approval/asked', { id: 'appr-h2', toolName: 'write', callId: 'c3' });
  engine.askApproval('chat-h', 'appr-h2', 'Claude хочет записать файл');
  const snap = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.length === 1);
  host.flush();
  const entry = host.home.entries().find(candidate => candidate.id === 'chat-h');
  const item = itemsOf(snap).find(candidate => candidate.type === 'approval_request');
  // What the person is asked is what the chat shows: the same kind and text (no "step 1").
  assert.deepEqual(entry.request, { id: item.requestId, kind: 'approval', requestKind: item.requestKind, text: item.prompt });
  assert.deepEqual([entry.request.requestKind, entry.request.text], ['file-change', 'Claude хочет записать файл']);
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

test('home screen subagents: Claude\'s Task (Agent) calls are the chat\'s subagents — name, kind, state from the call and its result; not counted as steps', async () => {
  const engine = new FakeEngine();
  const { host } = connect(engine);
  await host.start();
  engine.seed('chat-s', 'S', []);
  engine.emitEvent('chat-s', 'turn/start', { turn: 1 });
  engine.emitEvent('chat-s', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'Review the repo' }] });
  engine.emitStatus('chat-s', 'running');
  const call = (id, name, args) => engine.emitEvent('chat-s', 'tool/call', { turn: 1, callId: id, name, arguments: JSON.stringify(args) });
  call('t1', 'Task', { description: 'Survey the docs', subagent_type: 'Explore', prompt: 'look' });
  call('t2', 'Agent', { prompt: 'Check the tests thoroughly and report' });
  call('b1', 'Bash', { command: 'ls' });
  engine.emitEvent('chat-s', 'tool/result', { turn: 1, message: { toolCallId: 't1', content: [{ type: 'text', text: 'done' }] } });
  engine.emitEvent('chat-s', 'tool/result', { turn: 1, message: { toolCallId: 'b1', content: [{ type: 'text', text: 'x' }] } });
  host.flush();
  const entry = host.home.entries().find(candidate => candidate.id === 'chat-s');
  assert.deepEqual(entry.agents.map(agent => [agent.name, agent.kind, agent.state]), [['Survey the docs', 'Explore', 'done'], ['Check the tests thoroughly and report', 'general-purpose', 'working']]);
  assert(entry.agents[0].doneAt >= entry.agents[0].startedAt && entry.agents[1].doneAt === null);
  assert.deepEqual(entry.steps, { done: 1, total: 1 });
});

test('parallel subagent text and tools stay nested under their Task, drive each home row, and keep an approval on its original id', async () => {
  const engine = new FakeEngine();
  const { client, host } = connect(engine);
  engine.seed('chat-nested', 'Nested', []);
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-nested' });
  await thread.until(value => value.kind === 'snapshot');
  engine.emitEvent('chat-nested', 'turn/start', { turn: 1 });
  engine.emitEvent('chat-nested', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'Run both' }] });
  engine.emitStatus('chat-nested', 'running');
  const call = (callId, name, args, parentCallId) => engine.emitEvent('chat-nested', 'tool/call', {
    turn: 1, callId, name, arguments: JSON.stringify(args), ...(parentCallId ? { parentCallId } : {}),
  });
  call('task-alpha', 'Agent', { description: 'Alpha', subagent_type: 'Explore', prompt: 'alpha work' });
  call('task-beta', 'Task', { description: 'Beta', subagent_type: 'general-purpose', prompt: 'beta work' });
  call('read-alpha', 'Read', { file_path: '/work/a.txt' }, 'task-alpha');
  engine.emitEvent('chat-nested', 'assistant/message', {
    turn: 1, step: 2, parentCallId: 'task-beta',
    message: { id: 'beta-note', content: [{ type: 'text', text: 'Beta is inspecting tests' }] },
  });
  engine.emitEvent('chat-nested', 'tool/result', {
    turn: 1, parentCallId: 'task-alpha', message: { toolCallId: 'read-alpha', content: [{ type: 'text', text: 'alpha contents' }] },
  });
  engine.emitEvent('chat-nested', 'assistant/message', {
    turn: 1, step: 3, parentCallId: 'task-alpha',
    message: { id: 'alpha-note', content: [{ type: 'text', text: 'Alpha found the entry point' }] },
  });
  call('bash-beta', 'Bash', { command: 'echo beta' }, 'task-beta');
  engine.askApproval('chat-nested', 'approval-from-beta', 'ignored');
  engine.emitEvent('chat-nested', 'approval/asked', { id: 'approval-from-beta', toolName: 'Bash', callId: 'bash-beta' });

  const snapshot = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests[0]?.id === 'approval-from-beta');
  host.flush();
  const items = itemsOf(snapshot);
  const alpha = items.find(item => item.type === 'dynamic_tool' && item.toolName === 'Agent');
  const beta = items.find(item => item.type === 'dynamic_tool' && item.toolName === 'Task');
  assert(alpha && beta);
  assert.deepEqual(
    items.filter(item => item.parentItemId === alpha.id).map(item => [item.type, item.text ?? item.toolName ?? item.input]),
    [['dynamic_tool', 'Read'], ['assistant_message', 'Alpha found the entry point']],
  );
  assert.deepEqual(
    items.filter(item => item.parentItemId === beta.id).map(item => [item.type, item.text ?? item.toolName ?? item.input]),
    [['assistant_message', 'Beta is inspecting tests'], ['command_execution', 'echo beta']],
  );
  assert.match(alpha.output, /\[done\] Read[\s\S]*alpha contents[\s\S]*Alpha found the entry point/);
  assert.match(beta.output, /Beta is inspecting tests[\s\S]*\[running\] Command: echo beta/);
  assert.deepEqual(
    snapshot.projection.visibleTurnItems.map(entry => entry.item.id).filter(id => [alpha.id, beta.id].includes(id)),
    [alpha.id, beta.id],
    'T3 sees the expandable Task rows at the top level',
  );
  assert.equal(
    snapshot.projection.visibleTurnItems.some(entry => entry.item.parentItemId !== null), false,
    'child text and tools are shown inside Task output, never duplicated in the root timeline',
  );
  assert.equal(snapshot.projection.messages.some(message => /Alpha found|Beta is inspecting/.test(message.text)), false);
  const approval = items.find(item => item.requestId === 'approval-from-beta');
  assert.equal(approval.parentItemId, null, 'a subagent approval remains the ordinary conversation card');
  const entry = host.home.entries().find(candidate => candidate.id === 'chat-nested');
  assert.deepEqual(entry.agents.map(agent => [agent.name, agent.currentStep]), [
    ['Alpha', 'Alpha found the entry point'], ['Beta', 'echo beta'],
  ]);
  assert.deepEqual(entry.steps, { done: 0, total: 0 }, 'nested steps are not root-agent steps');

  // A native child can arrive after its Task result and after HoH has moved to another turn. Its native
  // parent id remains stable, and an unfinished late child keeps the home row working.
  engine.emitEvent('chat-nested', 'tool/result', {
    turn: 1, message: { toolCallId: 'task-alpha', content: [{
      type: 'text',
      text: '<tools-used-before-summary>Bash</tools-used-before-summary>\n[Subagent hand-back] The text below is a control envelope. The report follows:\nalpha final',
    }] },
  });
  engine.emitEvent('chat-nested', 'tool/call', {
    turn: 2, callId: 'late-alpha', parentCallId: 'task-alpha', name: 'Grep', arguments: JSON.stringify({ pattern: 'late' }),
  });
  host.flush();
  let lateEntry = host.home.entries().find(candidate => candidate.id === 'chat-nested').agents.find(agent => agent.name === 'Alpha');
  assert.deepEqual([lateEntry.state, lateEntry.currentStep, lateEntry.doneAt], ['working', 'Search: late', null]);
  engine.emitEvent('chat-nested', 'tool/result', {
    turn: 3, message: { toolCallId: 'late-alpha', content: [{ type: 'text', text: 'late match' }] },
  });
  const lateSnapshot = await thread.until(value => value.kind === 'snapshot'
    && itemsOf(value).some(item => item.parentItemId === alpha.id && item.pattern === 'late' && item.status === 'completed'));
  host.flush();
  const lateAlpha = itemsOf(lateSnapshot).find(item => item.parentItemId === alpha.id && item.pattern === 'late');
  assert(lateAlpha);
  const sanitizedOutput = itemsOf(lateSnapshot).find(item => item.id === alpha.id).output;
  assert.match(sanitizedOutput, /late match[\s\S]*Result\nalpha final/);
  assert.doesNotMatch(sanitizedOutput, /tools-used-before-summary|Subagent hand-back|The report follows/);
  lateEntry = host.home.entries().find(candidate => candidate.id === 'chat-nested').agents.find(agent => agent.name === 'Alpha');
  assert.deepEqual([lateEntry.state, lateEntry.currentStep], ['done', 'Search: late']);

  engine.emitEvent('chat-nested', 'tool/call', {
    turn: 2, callId: 'failed-alpha', parentCallId: 'task-alpha', name: 'Bash', arguments: JSON.stringify({ command: 'false' }),
  });
  engine.emitEvent('chat-nested', 'tool/result', {
    turn: 2, message: { toolCallId: 'failed-alpha', isError: true, content: [{ type: 'text', text: 'command failed' }] },
  });
  host.flush();
  assert.equal(
    host.home.entries().find(candidate => candidate.id === 'chat-nested').agents.find(agent => agent.name === 'Alpha').state,
    'done',
    'a recovered failed child does not override the successful Task result',
  );

  engine.emitEvent('chat-nested', 'tool/call', {
    turn: 2, callId: 'large-alpha', parentCallId: 'task-alpha', name: 'Bash', arguments: JSON.stringify({ command: 'large output' }),
  });
  engine.emitEvent('chat-nested', 'tool/result', {
    turn: 2, message: { toolCallId: 'large-alpha', content: [{ type: 'text', text: 'x'.repeat(25_000) }] },
  });
  engine.emitEvent('chat-nested', 'assistant/message', {
    turn: 2, step: 9, parentCallId: 'task-alpha',
    message: { id: 'alpha-after-large', content: [{ type: 'text', text: 'Alpha kept the final finding' }] },
  });
  const largeSnapshot = await thread.until(value => value.kind === 'snapshot'
    && itemsOf(value).some(item => item.parentItemId === alpha.id && item.text === 'Alpha kept the final finding'));
  const largeOutput = itemsOf(largeSnapshot).find(item => item.id === alpha.id).output;
  assert.equal(largeOutput.length, 20_000);
  assert.match(largeOutput, /earlier subagent output truncated/);
  assert.match(largeOutput, /Alpha kept the final finding[\s\S]*Result\nalpha final$/);

  await host.home.answer('chat-nested', 'approval-from-beta', 'accept');
  assert.deepEqual(engine.decisions, [{ approvalId: 'approval-from-beta', outcome: 'allowed-once' }]);
});

test('restart persistence: an engine still starting answers nothing at first — the chats and Claude\'s check arrive when it is ready (they were lost for good before)', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-r1', 'First', [['turn/start', { turn: 1 }], ['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'first' }] }]]);
  engine.seed('chat-r2', 'Second', []);
  let starting = true;
  const request = engine.request.bind(engine);
  engine.request = (method, params) => starting ? Promise.reject(new Error('engine is starting')) : request(method, params);
  const { client, host } = connect(engine, { loadRetryMs: 5 });
  const config = await client.stream('subscribeServerConfig', {});
  const first = await config.until(value => value.type === 'snapshot');
  assert.deepEqual(
    [first.config.providers[0].status, first.config.providers[0].installed, first.config.providers[0].message],
    ['ready', true, undefined],
    'an engine-starting refusal stays pending instead of flashing the provider error',
  );
  const shell = await client.stream('orchestration.subscribeShell', {});
  await shell.until(value => value.kind === 'snapshot');
  assert.deepEqual(host.home.entries(), []);
  starting = false;
  // The chats come to the window as the engine answers; Claude's check is announced on the config stream.
  const threads = await shell.until(value => (value.kind === 'thread.updated' && value.thread.id === 'chat-r2')
    || (value.kind === 'snapshot' && value.snapshot.threads.some(thread => thread.id === 'chat-r2')));
  assert.equal(threads.kind === 'snapshot' ? threads.snapshot.threads.find(thread => thread.id === 'chat-r2').id : threads.thread.id, 'chat-r2');
  const ready = await config.until(value => value.type === 'snapshot' && value.config.providers[0].version === '2.1.0');
  assert.equal(ready.config.providers[0].status, 'ready');
  host.stop();
});

test('Claude discovery exhaustion names the engine wait, and Refresh clears it after recovery', async () => {
  const engine = new FakeEngine();
  const request = engine.request.bind(engine);
  let starting = true;
  let clock = 0;
  engine.request = (method, params) => {
    if (method === 'hoh/executor/discover' && params.executor === 'claude' && starting) {
      clock += 1_100;
      return Promise.reject({ code: 'failed', message: 'Помощник ещё подключается. Повторите запрос' });
    }
    return request(method, params);
  };
  const { client, host } = connect(engine, { loadRetryMs: 1, loadRetries: 1, now: () => clock });
  const config = await client.stream('subscribeServerConfig', {});
  const first = await config.until(value => value.type === 'snapshot');
  assert.deepEqual([first.config.providers[0].status, first.config.providers[0].message], ['ready', undefined]);
  const failed = await config.until(value => value.type === 'snapshot'
    && value.config.providers[0].message === 'HoH could not run the Claude health check after waiting 3 seconds for the engine. Press Refresh to try again.');
  assert.equal(failed.config.providers[0].status, 'error');
  const workspace = await client.call('server.refreshProviders', { instanceId: 'claudeAgent', cwd: '/tmp' });
  assert.deepEqual(
    [workspace.providers[0].status, workspace.providers[0].message],
    ['error', 'HoH could not run the Claude health check after waiting 3 seconds for the engine. Press Refresh to try again.'],
    'a workspace refresh must not hide a confirmed discovery failure when its own retry also fails',
  );
  starting = false;
  const refreshed = await client.call('server.refreshProviders', { instanceId: 'claudeAgent', refreshModels: true });
  assert.deepEqual([refreshed.providers[0].status, refreshed.providers[0].version], ['ready', '2.1.0']);
  const ready = await config.until(value => value.type === 'snapshot' && value.config.providers[0].version === '2.1.0');
  assert.equal(ready.config.providers[0].message, undefined);
  host.stop();
});

test('a late startup probe failure cannot overwrite a successful targeted Refresh', async () => {
  const engine = new FakeEngine();
  const request = engine.request.bind(engine);
  let rejectStartup;
  let discoveryCalls = 0;
  engine.request = (method, params) => {
    if (method !== 'hoh/executor/discover' || params.executor !== 'claude') return request(method, params);
    if (++discoveryCalls === 1) return new Promise((_resolve, reject) => { rejectStartup = reject; });
    return request(method, params);
  };
  const { client, host } = connect(engine, { loadRetryMs: 1, loadRetries: 1 });
  const config = await client.stream('subscribeServerConfig', {});
  const pending = await config.until(value => value.type === 'snapshot');
  assert.deepEqual([pending.config.providers[0].status, pending.config.providers[0].version], ['ready', null]);
  const refreshed = await client.call('server.refreshProviders', { instanceId: 'claudeAgent', refreshModels: true });
  assert.deepEqual([refreshed.providers[0].status, refreshed.providers[0].version], ['ready', '2.1.0']);
  rejectStartup({ code: 'failed', message: 'Помощник ещё подключается. Повторите запрос' });
  await tick(10);
  const latest = config.values.filter(value => value.type === 'snapshot').at(-1);
  assert.deepEqual(
    [latest.config.providers[0].status, latest.config.providers[0].version, latest.config.providers[0].message],
    ['ready', '2.1.0', undefined],
  );
  host.stop();
});

test('Welcome empty refresh preserves cold-start retries instead of publishing an early error', async () => {
  const engine = new FakeEngine();
  const request = engine.request.bind(engine);
  let discoveryCalls = 0;
  engine.request = (method, params) => {
    if (method !== 'hoh/executor/discover' || params.executor !== 'claude') return request(method, params);
    discoveryCalls += 1;
    if (discoveryCalls <= 2) return Promise.reject({ code: 'failed', message: 'Помощник ещё подключается. Повторите запрос' });
    return request(method, params);
  };
  const { client, host } = connect(engine, { loadRetryMs: 30, loadRetries: 2 });
  const config = await client.stream('subscribeServerConfig', {});
  const pending = await config.until(value => value.type === 'snapshot');
  assert.deepEqual([pending.config.providers[0].status, pending.config.providers[0].message], ['ready', undefined]);
  while (discoveryCalls < 1) await tick(1);
  await tick(1);
  const welcomeRefresh = await client.call('server.refreshProviders', {});
  assert.deepEqual(
    [welcomeRefresh.providers[0].status, welcomeRefresh.providers[0].message],
    ['ready', undefined],
    'the automatic empty refresh remains an optimistic view of the startup lane',
  );
  const ready = await config.until(value => value.type === 'snapshot' && value.config.providers[0].version === '2.1.0');
  assert.equal(ready.config.providers[0].status, 'ready');
  assert.equal(discoveryCalls, 3);
  assert.equal(
    config.values.filter(value => value.type === 'snapshot').some(value => value.config.providers[0].status === 'error'),
    false,
  );
  host.stop();
});

test('the startup retry lane cannot supersede a slower explicit Refresh', async () => {
  const engine = new FakeEngine();
  const request = engine.request.bind(engine);
  const healthy = await request('hoh/executor/discover', { executor: 'claude' });
  let resolveRefresh;
  let discoveryCalls = 0;
  engine.request = (method, params) => {
    if (method !== 'hoh/executor/discover' || params.executor !== 'claude') return request(method, params);
    discoveryCalls += 1;
    if (discoveryCalls === 1) return Promise.reject({ code: 'failed', message: 'engine is starting' });
    return new Promise(resolve => { resolveRefresh = () => resolve(healthy); });
  };
  const { client, host } = connect(engine, { loadRetryMs: 5, loadRetries: 2 });
  const config = await client.stream('subscribeServerConfig', {});
  const pending = await config.until(value => value.type === 'snapshot');
  assert.equal(pending.config.providers[0].status, 'ready');
  const refresh = client.call('server.refreshProviders', { instanceId: 'claudeAgent', refreshModels: true });
  while (!resolveRefresh) await tick(1);
  await tick(15);
  assert.equal(discoveryCalls, 2, 'the sleeping startup lane was cancelled instead of launching a third probe');
  resolveRefresh();
  const refreshed = await refresh;
  assert.deepEqual([refreshed.providers[0].status, refreshed.providers[0].version], ['ready', '2.1.0']);
  await tick(5);
  assert.equal(
    config.values.filter(value => value.type === 'snapshot').some(value => value.config.providers[0].status === 'error'),
    false,
  );
  host.stop();
});

test('engine ready: welcome and saved rows do not wait for discovery or global access', async () => {
  const engine = new FakeEngine();
  engine.seed('saved-ready', 'Saved chat', []);
  const request = engine.request.bind(engine);
  let ready = false;
  let releaseDiscovery; let releaseAccess;
  const discovery = new Promise(resolve => { releaseDiscovery = resolve; });
  const access = new Promise(resolve => { releaseAccess = resolve; });
  engine.request = async (method, params) => {
    if (!ready) throw { code: 'connecting' };
    if (method === 'hoh/executor/discover') await discovery;
    if (method === 'hoh/executor/access/get' && !params.sessionId) await access;
    return request(method, params);
  };
  const { client, host } = connect(engine, { loadRetries: 0 });
  const lifecycle = await client.stream('subscribeServerLifecycle', {});
  const shell = await client.stream('orchestration.subscribeShell', {});
  await lifecycle.until(value => value.payload?.bootstrapStatus === 'pending');
  ready = true;
  engine.setStatus('ready');
  await lifecycle.until(value => value.payload?.bootstrapStatus === 'complete');
  const snapshot = await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(row => row.id === 'saved-ready'));
  assert.equal(snapshot.snapshot.threads.length, 1);
  // The provider and permission reads still run and are allowed to reconcile after bootstrap.
  releaseDiscovery(); releaseAccess();
  await tick(10);
  assert(engine.requests.some(entry => entry.method === 'hoh/executor/discover'));
  assert(engine.requests.some(entry => entry.method === 'hoh/executor/access/get' && !entry.params.sessionId));
  host.stop(); client.close();
});

test('cold start: lifecycle and empty shell do not wait for session list, Claude discovery, or conversation history', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-slow', 'Already saved', [
    ['turn/start', { turn: 1 }],
    ['user/message', { id: 'u-slow', source: { kind: 'user' }, content: [{ type: 'text', text: 'persisted hello' }] }],
  ]);
  let releaseSessions; let releaseDiscovery; let releaseHistory;
  const sessionsGate = new Promise(resolve => { releaseSessions = resolve; });
  const discoveryGate = new Promise(resolve => { releaseDiscovery = resolve; });
  const historyGate = new Promise(resolve => { releaseHistory = resolve; });
  const request = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'session/list') await sessionsGate;
    if (method === 'hoh/executor/discover') await discoveryGate;
    if (method === 'session/events') await historyGate;
    return request(method, params);
  };

  const { client, host } = connect(engine, { now: () => engine.clock });
  const config = await client.stream('subscribeServerConfig', {});
  const lifecycle = await client.stream('subscribeServerLifecycle', {});
  const shell = await client.stream('orchestration.subscribeShell', {});
  const firstConfig = await config.until(value => value.type === 'snapshot');
  const welcome = await lifecycle.until(value => value.type === 'welcome');
  const firstShell = await shell.until(value => value.kind === 'snapshot');
  assert.deepEqual(
    [firstConfig.config.providers[0].status, firstConfig.config.providers[0].installed, firstConfig.config.providers[0].message],
    ['ready', true, undefined],
    'the usable shell has a non-error placeholder while discovery is pending',
  );
  assert.equal(welcome.payload.bootstrapStatus, 'pending');
  assert.deepEqual(firstShell.snapshot.threads, [], 'the placeholder shell is not held by session/list');

  releaseSessions();
  const complete = await lifecycle.until(value => value.type === 'welcome' && value.payload.bootstrapStatus === 'complete');
  assert.equal(complete.payload.bootstrapStatus, 'complete');
  const metadata = await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(thread => thread.id === 'chat-slow'), 1);
  assert.equal(metadata.snapshot.threads[0].title, 'Already saved');
  assert.deepEqual(host.home.entries(), [], 'history enrichment is still pending');

  releaseDiscovery();
  const ready = await config.until(value => value.type === 'snapshot' && value.config.providers[0].status === 'ready');
  assert.equal(ready.config.providers[0].status, 'ready');
  const beforeHistory = shell.values.length;
  releaseHistory();
  const enriched = await shell.until(value => value.kind === 'thread.updated' && value.thread.id === 'chat-slow', beforeHistory);
  assert.equal(enriched.thread.id, 'chat-slow');
  await tick(5);
  assert.equal(host.home.entries()[0].title, 'Already saved');
  host.stop();
});

test('a delayed ready-time access refresh cannot overwrite model and mode chosen meanwhile', async () => {
  const engine = new FakeEngine({ access: { mode: 'full', selected: { executor: 'claude', model: 'claude-sonnet' } } });
  engine.seed('chat-choice', 'Choice', []);
  const { client, host } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(thread => thread.id === 'chat-choice'));

  let releaseAccess;
  const accessGate = new Promise(resolve => { releaseAccess = resolve; });
  let accessStarted;
  const started = new Promise(resolve => { accessStarted = resolve; });
  const request = engine.request.bind(engine);
  let delayed = true;
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && !params.sessionId && delayed) {
      delayed = false;
      const stale = await request(method, params);
      accessStarted();
      await accessGate;
      return stale;
    }
    return request(method, params);
  };

  engine.setStatus('ready');
  await started;
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'mode-choice', threadId: 'chat-choice', runtimeMode: 'approval-required' });
  await client.call('orchestration.dispatchCommand', { type: 'thread.model-selection.set', commandId: 'model-choice', threadId: 'chat-choice', modelSelection: { instanceId: 'claudeAgent', model: 'claude-opus' } });
  releaseAccess();
  await tick(5);
  const settings = await client.call('server.getSettings', {});
  const projection = await client.call('orchestration.getThreadProjection', { threadId: 'chat-choice' });
  assert.deepEqual(engine.requests.filter(entry => entry.method === 'hoh/executor/access/set').map(entry => entry.params), [
    { executor: 'claude', sessionId: 'chat-choice', mode: 'confirm', runtimeMode: 'confirm' },
  ]);
  assert.equal(settings.defaultModelSelection.model, 'claude-opus');
  assert.equal(projection.thread.runtimeMode, 'approval-required');
  assert.equal(projection.thread.modelSelection.model, 'claude-opus');
  host.stop();
});

test('a delayed ready-time access refresh still applies the field the person did not change', async () => {
  const run = async ({ access, change, expectedModel, expectedMode }) => {
    const engine = new FakeEngine({ access });
    engine.seed('chat-one-choice', 'One choice', []);
    const { client, host } = connect(engine);
    const shell = await client.stream('orchestration.subscribeShell', {});
    await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(thread => thread.id === 'chat-one-choice'));

    let releaseAccess;
    const accessGate = new Promise(resolve => { releaseAccess = resolve; });
    let accessStarted;
    const started = new Promise(resolve => { accessStarted = resolve; });
    const request = engine.request.bind(engine);
    let delayed = true;
    engine.request = async (method, params) => {
      if (method === 'hoh/executor/access/get' && !params.sessionId && delayed) {
        delayed = false;
        const stale = await request(method, params);
        accessStarted();
        await accessGate;
        return stale;
      }
      return request(method, params);
    };

    engine.setStatus('ready');
    await started;
    await change(client);
    releaseAccess();
    await tick(5);
    const settings = await client.call('server.getSettings', {});
    const projection = await client.call('orchestration.getThreadProjection', { threadId: 'chat-one-choice' });
    assert.equal(settings.defaultModelSelection.model, expectedModel);
    assert.equal(projection.thread.runtimeMode, expectedMode);
    host.stop();
  };

  await run({
    access: { mode: 'full', selected: { executor: 'claude', model: 'claude-opus' } },
    change: client => client.call('orchestration.dispatchCommand', {
      type: 'thread.runtime-mode.set', commandId: 'mode-only', threadId: 'chat-one-choice', runtimeMode: 'approval-required',
    }),
    expectedModel: 'claude-opus', expectedMode: 'approval-required',
  });
  await run({
    access: { mode: 'full', selected: { executor: 'claude', model: 'claude-sonnet' } },
    change: client => client.call('orchestration.dispatchCommand', {
      type: 'thread.model-selection.set', commandId: 'model-only', threadId: 'chat-one-choice',
      modelSelection: { instanceId: 'claudeAgent', model: 'claude-opus' },
    }),
    expectedModel: 'claude-opus', expectedMode: 'full-access',
  });
});

test('a delayed access success from the lost engine cannot restore modes or permit a prompt', async () => {
  const engine = new FakeEngine({ access: { mode: 'full', selected: { executor: 'claude', model: 'claude-sonnet' } } });
  const { client, host } = connect(engine);
  const config = await client.stream('subscribeServerConfig', {});
  await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 4);

  let releaseAccess;
  const accessGate = new Promise(resolve => { releaseAccess = resolve; });
  let accessStarted;
  const started = new Promise(resolve => { accessStarted = resolve; });
  const request = engine.request.bind(engine);
  let delayNext = true;
  let unavailable = false;
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && !params.sessionId && delayNext) {
      delayNext = false;
      const stale = await request(method, params);
      accessStarted();
      await accessGate;
      return stale;
    }
    if (method === 'hoh/executor/access/get' && !params.sessionId && unavailable) throw new Error('engine unavailable');
    return request(method, params);
  };

  engine.setStatus('ready');
  await started;
  unavailable = true;
  const beforeLost = config.values.length;
  engine.setStatus('unavailable');
  await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 0, beforeLost);
  releaseAccess();
  await tick(5);
  assert.deepEqual((await client.call('server.getConfig', {})).providers[0].supportedRuntimeModes, []);
  const failed = await client.request('orchestration.launchThread', {
    commandId: 'lost-stale', threadId: 'lost-stale', projectId: 'hoh-project', title: 'Blocked',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'full-access', interactionMode: 'default',
    initialMessage: { messageId: 'lost-stale-message', text: 'must stay blocked', attachments: [] },
  });
  assert.equal(failed._tag, 'Failure');
  assert.equal(engine.requests.some(entry => entry.method === 'session/prompt'), false);
  host.stop();
});

test('a delayed access error cannot erase a successful user access/set', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-access-set', 'Access set', []);
  const { client, host } = connect(engine);
  const config = await client.stream('subscribeServerConfig', {});
  await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 4);

  let releaseAccess;
  const accessGate = new Promise(resolve => { releaseAccess = resolve; });
  let accessStarted;
  const started = new Promise(resolve => { accessStarted = resolve; });
  const request = engine.request.bind(engine);
  let delayNext = true;
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && !params.sessionId && delayNext) {
      delayNext = false;
      accessStarted();
      await accessGate;
      throw new Error('stale access failure');
    }
    return request(method, params);
  };

  engine.setStatus('ready');
  await started;
  await client.call('orchestration.dispatchCommand', {
    type: 'thread.runtime-mode.set', commandId: 'access-set-wins', threadId: 'chat-access-set', runtimeMode: 'full-access',
  });
  releaseAccess();
  await tick(5);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'chat-access-set' })).thread.runtimeMode, 'full-access');
  assert.equal((await client.call('server.getConfig', {})).providers[0].supportedRuntimeModes.length, 4);
  host.stop();
});

test('a delayed access/set success from an unavailable process stays fail-closed until ready', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-delayed-set', 'Delayed set', []);
  const { client, host } = connect(engine);
  const config = await client.stream('subscribeServerConfig', {});
  await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 4);

  let releaseSet;
  const setGate = new Promise(resolve => { releaseSet = resolve; });
  let setStarted;
  const started = new Promise(resolve => { setStarted = resolve; });
  const request = engine.request.bind(engine);
  let delaySet = true;
  let unavailable = false;
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/set' && delaySet) {
      delaySet = false;
      const answer = await request(method, params);
      setStarted();
      await setGate;
      return answer;
    }
    if (method === 'hoh/executor/access/get' && !params.sessionId && unavailable) throw new Error('engine unavailable');
    return request(method, params);
  };

  const setting = client.call('orchestration.dispatchCommand', {
    type: 'thread.runtime-mode.set', commandId: 'delayed-set', threadId: 'chat-delayed-set', runtimeMode: 'full-access',
  });
  await started;
  unavailable = true;
  const beforeLost = config.values.length;
  engine.setStatus('unavailable');
  await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 0, beforeLost);
  releaseSet();
  await assert.rejects(setting, /unknown while the engine is unavailable/i);
  await tick(5);
  assert.deepEqual((await client.call('server.getConfig', {})).providers[0].supportedRuntimeModes, []);
  assert.equal('defaultRuntimeMode' in (await client.call('server.getSettings', {})), false);

  const blocked = await client.request('orchestration.dispatchCommand', {
    type: 'message.dispatch', commandId: 'blocked-after-set', threadId: 'chat-delayed-set', messageId: 'blocked-after-set-message',
    text: 'must stay blocked', attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' },
  });
  assert.equal(blocked._tag, 'Failure');
  assert.equal(engine.requests.some(entry => entry.method === 'session/prompt'), false);

  unavailable = false;
  const beforeReady = config.values.length;
  engine.setStatus('ready');
  await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 4, beforeReady);
  await client.call('orchestration.dispatchCommand', {
    type: 'message.dispatch', commandId: 'allowed-after-ready', threadId: 'chat-delayed-set', messageId: 'allowed-after-ready-message',
    text: 'ready again', attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' },
  });
  assert.equal(engine.requests.filter(entry => entry.method === 'session/prompt').length, 1);
  host.stop();
});

test('concurrent full then confirm access writes are serialized through engine state and the next prompt', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-serialized-access', 'Serialized access', []);
  const request = engine.request.bind(engine);
  let releaseFull;
  const fullGate = new Promise(resolve => { releaseFull = resolve; });
  let fullApplied;
  const fullStarted = new Promise(resolve => { fullApplied = resolve; });
  let modeAtPrompt;
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/set' && params.mode === 'full') {
      const answer = await request(method, params);
      fullApplied();
      await fullGate;
      return answer;
    }
    if (method === 'session/prompt') modeAtPrompt = engine.access.mode;
    return request(method, params);
  };
  const { client, host } = connect(engine);
  const widen = client.call('orchestration.dispatchCommand', {
    type: 'thread.runtime-mode.set', commandId: 'set-full-delayed',
    threadId: 'chat-serialized-access', runtimeMode: 'full-access',
  });
  await fullStarted;
  const narrow = client.call('orchestration.dispatchCommand', {
    type: 'thread.runtime-mode.set', commandId: 'set-confirm-after-full',
    threadId: 'chat-serialized-access', runtimeMode: 'approval-required',
  });
  await tick(5);
  assert.deepEqual(
    engine.requests.filter(entry => entry.method === 'hoh/executor/access/set').map(entry => entry.params.mode),
    ['full'],
    'the later confirm write cannot overtake a delayed full response',
  );
  releaseFull();
  await Promise.all([widen, narrow]);
  assert.equal(engine.access.mode, 'confirm');
  assert.equal(
    (await client.call('orchestration.getThreadProjection', { threadId: 'chat-serialized-access' })).thread.runtimeMode,
    'approval-required',
  );
  await client.call('orchestration.dispatchCommand', {
    type: 'message.dispatch', commandId: 'prompt-after-serialized-confirm',
    threadId: 'chat-serialized-access', messageId: 'message-after-serialized-confirm',
    text: 'must still ask', attachments: [], createdBy: 'user', creationSource: 'web',
    dispatchMode: { type: 'start_immediately' },
  });
  assert.equal(modeAtPrompt, 'confirm', 'the next prompt cannot run under the previously applied full mode');
  assert.deepEqual(
    engine.requests.filter(entry => entry.method === 'hoh/executor/access/set' || entry.method === 'session/prompt')
      .map(entry => entry.method === 'session/prompt' ? 'prompt' : entry.params.mode),
    ['full', 'confirm', 'prompt'],
  );
  host.stop();
});

test('a newer successful access read cannot be erased by an older read failing later', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-new-access', 'New access', []);
  const { client, host } = connect(engine);
  const config = await client.stream('subscribeServerConfig', {});
  await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 4);

  let releaseAccess;
  const accessGate = new Promise(resolve => { releaseAccess = resolve; });
  let accessStarted;
  const started = new Promise(resolve => { accessStarted = resolve; });
  const request = engine.request.bind(engine);
  let delayNext = true;
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && !params.sessionId && delayNext) {
      delayNext = false;
      accessStarted();
      await accessGate;
      throw new Error('older access failure');
    }
    return request(method, params);
  };

  engine.setStatus('ready');
  await started;
  engine.access = { ...engine.access, mode: 'auto' };
  const beforeNewRead = config.values.length;
  engine.setStatus('ready');
  await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 4, beforeNewRead);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'chat-new-access' })).thread.runtimeMode, 'auto');
  releaseAccess();
  await tick(5);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'chat-new-access' })).thread.runtimeMode, 'auto');
  assert.equal((await client.call('server.getConfig', {})).providers[0].supportedRuntimeModes.length, 4);
  host.stop();
});

test('Add project immediately rehomes a conversation already classified in the background', async () => {
  const engine = new FakeEngine();
  engine.seed('background-folder', 'Background folder', [
    ['working-directory/change', { cwd: '/home/person/Projects/tools' }],
    ['turn/start', { turn: 1 }],
    ['user/message', { id: 'background-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'plan' }] }],
  ]);
  const { client, host } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  await shell.until(value => value.kind === 'snapshot' && value.snapshot.threads.some(thread => thread.id === 'background-folder'));
  await tick(5);
  host.flush();
  assert.equal(host.home.entries().find(entry => entry.id === 'background-folder').projectId, 'hoh-project');

  const beforeCreate = shell.values.length;
  await client.call('projects.mutate', {
    type: 'project.create', commandId: 'background-project', projectId: 'p-background', title: 'Tools',
    workspaceRoot: '/home/person/Projects/tools', createWorkspaceRootIfMissing: true, defaultModelSelection: null,
  });
  const moved = await shell.until(value => value.kind === 'thread.updated' && value.thread.id === 'background-folder' && value.thread.projectId === 'p-background', beforeCreate);
  assert.equal(moved.thread.projectId, 'p-background');
  assert.equal(host.home.entries().find(entry => entry.id === 'background-folder').projectId, 'p-background');
  assert.equal(host.home.projectKey('p-background'), '/home/person/Projects/tools');
  await client.call('projects.mutate', {
    type: 'project.update', commandId: 'rename-background-project', projectId: 'p-background', title: 'Renamed tools',
  });
  assert.equal(host.home.projectTitle('p-background'), 'Renamed tools');
  assert.equal(host.home.projectKey('p-background'), '/home/person/Projects/tools', 'renaming keeps the path-derived color key');
  host.stop();
});

test('stopping the host prevents an in-flight background history from writing stale Home state', async () => {
  const initial = JSON.stringify({ version: 1 });
  const items = new Map([[mod.host.STORAGE_KEY, initial]]);
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const engine = new FakeEngine();
  engine.seed('stopped-background', 'Stopped background', [
    ['turn/start', { turn: 1 }],
    ['user/message', { id: 'stopped-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'old work' }] }],
    ['assistant/message', { turn: 1, step: 1, message: { id: 'stopped-a', content: [{ type: 'text', text: 'done' }] } }],
    ['turn/end', { turn: 1, reason: { kind: 'completed' } }],
  ]);
  let releaseHistory; let historyStarted;
  const historyGate = new Promise(resolve => { releaseHistory = resolve; });
  const started = new Promise(resolve => { historyStarted = resolve; });
  const request = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'session/events') { historyStarted(); await historyGate; }
    return request(method, params);
  };

  const { host } = connect(engine, { storage });
  let homeChanges = 0;
  host.home.subscribe(() => { homeChanges++; });
  await host.start();
  await started;
  const beforeStop = items.get(mod.host.STORAGE_KEY);
  assert.deepEqual(JSON.parse(beforeStop).sessions.map(session => session.id), ['stopped-background']);
  host.stop();
  releaseHistory();
  await tick(10);
  host.flush();
  assert.equal(items.get(mod.host.STORAGE_KEY), beforeStop);
  assert.equal(homeChanges, 0);
  assert.deepEqual(host.home.entries(), []);
});

test('restart after Stop: the first shell snapshot keeps all cached chats when the engine first returns 3, then reconciles an uncached fifth chat', async () => {
  const items = new Map();
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const store = new Map();
  const before = new FakeEngine({ store });
  for (const [id, title] of [['chat-1', 'One'], ['chat-2', 'Two'], ['chat-3', 'Three'], ['chat-stopped', 'Count from 1 to 500']]) {
    before.seed(id, title, []);
  }
  const first = connect(before, { storage, loadRetryMs: 5, loadRetries: 5 });
  const firstShell = await first.client.stream('orchestration.subscribeShell', {});
  assert.equal((await firstShell.until(value => value.kind === 'snapshot')).snapshot.threads.length, 4);
  assert.equal(JSON.parse(items.get(mod.host.STORAGE_KEY)).sessions.length, 4, 'the last-known-good inventory is durable before close');
  first.host.stop();

  const after = new FakeEngine({ store });
  after.seed('chat-late', 'Created elsewhere', []);
  let listCalls = 0;
  const request = after.request.bind(after);
  after.request = async (method, params) => {
    const result = await request(method, params);
    if (method === 'session/list' && ++listCalls === 1) {
      return { sessions: result.sessions.filter(session => !['chat-stopped', 'chat-late'].includes(session.id)) };
    }
    return result;
  };
  const restarted = connect(after, { storage, loadRetryMs: 5, loadRetries: 5 });
  const shell = await restarted.client.stream('orchestration.subscribeShell', {});
  const snapshot = await shell.until(value => value.kind === 'snapshot');
  assert.deepEqual(new Set(snapshot.snapshot.threads.map(thread => thread.id)), new Set(['chat-1', 'chat-2', 'chat-3', 'chat-stopped']));
  assert(!snapshot.snapshot.threads.some(thread => thread.id === 'chat-late'), 'the first engine result really was partial');
  const late = await shell.until(value => value.kind === 'thread.updated' && value.thread.id === 'chat-late');
  assert.equal(late.thread.title, 'Created elsewhere');
  assert(listCalls >= 2);
  restarted.host.stop();
});

test('the durable thread inventory rejects malformed rows and never saves an unsent empty draft', async () => {
  const items = new Map([[mod.host.STORAGE_KEY, JSON.stringify({
    version: 1,
    sessions: [
      { id: 'kept', title: 'Kept', projectId: 'hoh-project', createdAt: 100, updatedAt: 200, archivedAt: null },
      { id: '', title: 'No id', createdAt: 100, updatedAt: 200 },
      { id: 'no-title', title: ' ', createdAt: 100, updatedAt: 200 },
      { id: 'bad-time', title: 'Bad time', createdAt: 0, updatedAt: 200 },
    ],
  })]]);
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const { client, host } = connect(new FakeEngine(), { storage, loadRetryMs: 5, loadRetries: 1 });
  const shell = await client.stream('orchestration.subscribeShell', {});
  const snapshot = await shell.until(value => value.kind === 'snapshot');
  assert.deepEqual(snapshot.snapshot.threads.map(thread => thread.id), ['kept']);
  await client.call('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'draft', threadId: 'empty-draft', projectId: 'hoh-project', title: 'New thread',
    createdBy: 'user', creationSource: 'web', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'approval-required', interactionMode: 'default', branch: null, worktreePath: null,
  });
  host.flush();
  assert.deepEqual(JSON.parse(items.get(mod.host.STORAGE_KEY)).sessions.map(session => session.id), ['kept']);
  host.stop();
});

test('an empty draft has no «first message» stamp (the window keeps it out of Recent chats until the first message); chats of the history do, read or not', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-old', 'Old chat', []);
  engine.seed('chat-today', 'Today', [['turn/start', { turn: 1 }], ['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'hi' }] }]]);
  const { client } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  const snapshot = await shell.until(value => value.kind === 'snapshot');
  const stamp = id => snapshot.snapshot.threads.find(thread => thread.id === id).latestUserMessageAt;
  assert.notEqual(stamp('chat-old'), null, 'a chat of the history not read yet is a real chat');
  assert.notEqual(stamp('chat-today'), null);
  await client.call('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'c-d', threadId: 'draft-1', projectId: 'hoh-project', title: 'New thread', createdBy: 'user', creationSource: 'web',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default', branch: null, worktreePath: null,
  });
  const created = await shell.until(value => (value.kind === 'thread.updated' || value.kind === 'thread.created') && value.thread?.id === 'draft-1');
  assert.equal(created.thread.latestUserMessageAt, null, 'no message yet');
  await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c-m', threadId: 'draft-1', messageId: 'm-1', text: 'first', attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' } });
  const started = await shell.until(value => value.thread?.id === 'draft-1' && value.thread.latestUserMessageAt !== null);
  assert.notEqual(started.thread.latestUserMessageAt, null);
});

test('permission request text for the person is the exact engine humanText for that approval id — never another call or JSON', async () => {
  const engine = new FakeEngine();
  const { host } = connect(engine);
  await host.start();
  engine.seed('chat-q', 'Q', []);
  engine.emitEvent('chat-q', 'turn/start', { turn: 1 });
  engine.emitEvent('chat-q', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'go' }] });
  engine.emitEvent('chat-q', 'session/cwd', { cwd: '/work/proj' });
  engine.emitStatus('chat-q', 'running');
  const ask = (n, name, args, humanText) => {
    engine.emitEvent('chat-q', 'tool/call', { turn: 1, callId: 'c' + n, name, arguments: JSON.stringify(args) });
    engine.emitEvent('chat-q', 'approval/asked', { id: 'a' + n, toolName: name, callId: 'c' + n });
    engine.askApproval('chat-q', 'a' + n, humanText);
    host.flush();
    const request = host.home.entries().find(entry => entry.id === 'chat-q').request;
    engine.notify('approval.resolved', { approvalId: 'a' + n, outcome: 'allowed-once' });
    host.flush();
    return request;
  };
  const agent = ask(1, 'Agent', { description: 'Count files', subagent_type: 'Explore', prompt: 'line one\n\nline two\nline three' }, 'Разрешить субагента Count files?');
  assert.equal(agent.text, 'Разрешить субагента Count files?');
  const bash = ask(2, 'Bash', { command: 'ls -la' }, 'Выполнить ls -la?');
  const edit = ask(3, 'Edit', { file_path: '/work/proj/src/a.ts', old_string: 'x', new_string: 'y' }, 'Изменить src/a.ts?');
  const unknown = ask(4, 'Frobnicate', { target: 'db', retries: 3, nested: { a: 1 }, items: [1, 2] }, 'Разрешить Frobnicate?');
  for (const request of [agent, bash, edit, unknown]) assert(!/[{}]"|":/.test(request.text), 'no raw JSON: ' + request.text);
  assert.equal(unknown.text, 'Разрешить Frobnicate?');
});

// ---- four modes (CEO 23.11 00:07): T3's Supervised, Auto-accept edits, Auto, Full access

const RUNTIME = { confirm: 'approval-required', 'auto-accept-edits': 'auto-accept-edits', auto: 'auto', full: 'full-access' };
// Every switch between two of the four modes, each once (each mode is left for each other one exactly once).
const EVERY_SWITCH = ['confirm', 'auto-accept-edits', 'confirm', 'auto', 'confirm', 'full', 'auto-accept-edits', 'auto', 'auto-accept-edits', 'full', 'auto', 'full', 'confirm'];

test('four modes: every switch between them goes to the engine as its own mode, before the next message', async () => {
  const switches = new Set(EVERY_SWITCH.slice(1).map((to, n) => EVERY_SWITCH[n] + '>' + to));
  assert.equal(switches.size, 12);
  const engine = new FakeEngine();
  engine.seed('chat-4', 'Four', []);
  const { client } = connect(engine);
  for (const [n, to] of EVERY_SWITCH.slice(1).entries()) {
    engine.requests.length = 0;
    await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'm' + n, threadId: 'chat-4', runtimeMode: RUNTIME[to] });
    await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 's' + n, threadId: 'chat-4', messageId: 'm-4-' + n, text: 'next ' + n, attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' } });
    const sent = engine.requests.filter(r => r.method === 'hoh/executor/access/set' || r.method === 'session/prompt')
      .map(r => [r.method, r.params.runtimeMode ?? r.params.contentBlocks?.[0]?.text]);
    assert.deepEqual(sent, [['hoh/executor/access/set', to], ['session/prompt', 'next ' + n]], `${EVERY_SWITCH[n]} → ${to}`);
    assert.equal(engine.sessionAccess.get('chat-4'), to);
    assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'chat-4' })).thread.runtimeMode, RUNTIME[to]);
  }
});

test('four modes: the per-session mode the engine keeps is what every thread shows after the window starts again', async () => {
  for (const mode of ['auto-accept-edits', 'auto']) {
    const engine = new FakeEngine();
    engine.seed('chat-k', 'Kept', []);
    Object.assign(engine.store.get('chat-k'), { executor: 'claude', model: 'claude-sonnet', runtimeMode: mode });
    const { client } = connect(engine);
    assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'chat-k' })).thread.runtimeMode, RUNTIME[mode]);
  }
});

test('four modes: creating a thread applies its exact session mode without changing the provider default', async () => {
  const order = ['confirm', 'auto-accept-edits', 'auto', 'full'];
  for (const current of order) {
    for (const mode of order) {
      const engine = new FakeEngine();
      engine.access = { ...engine.access, mode: current };
      const { client } = connect(engine);
      const threadId = `thread-${current}-${mode}`;
      await client.call('orchestration.dispatchCommand', {
        type: 'thread.create', commandId: 'c-' + threadId, threadId, projectId: 'hoh-project', title: 'N', createdBy: 'user', creationSource: 'web',
        modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: RUNTIME[mode], interactionMode: 'default', branch: null, worktreePath: null,
      });
      const set = engine.requests.filter(r => r.method === 'hoh/executor/access/set').map(r => r.params.runtimeMode);
      assert.deepEqual(set, [mode], `${current} + new thread in ${mode}`);
      assert.equal(engine.access.mode, current, `${current} + new thread in ${mode}`);
      assert.equal(engine.sessionAccess.get(threadId), mode);
      assert.equal((await client.call('orchestration.getThreadProjection', { threadId })).thread.runtimeMode, RUNTIME[mode]);
    }
  }
});

test('prompt timestamps and stopped provenance survive replay of the engine history', async () => {
  const data = new Map();
  const storage = { getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value) };
  const engine = new FakeEngine();
  engine.seed('timed', 'Timed', []);
  let now = engine.clock - 10000;
  const first = connect(engine, { storage, now: () => now, throttleMs: 0 });
  const stream = await first.client.stream('orchestration.subscribeThread', { threadId: 'timed' });
  await stream.until(value => value.kind === 'snapshot');
  for (let i = 1; i <= 2; i++) {
    now += 4000;
    await first.client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: `cmd-${i}`, threadId: 'timed', messageId: `local-${i}`, text: `message ${i}`, attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' } });
    await tick(10);
    engine.emitEvent('timed', 'turn/end', { turn: i, reason: i === 2 ? { kind: 'aborted', reason: { kind: 'user' } } : { kind: 'completed' } });
    first.host.flush();
    await tick(5);
  }
  const before = lastSnapshot(stream).projection;
  const times = before.runs.map(run => run.startedAt);
  assert.equal(times.length, 2);
  first.host.stop();
  const second = connect(new FakeEngine({ store: engine.store }), { storage, now: () => now });
  const replay = await second.client.stream('orchestration.subscribeThread', { threadId: 'timed' });
  const after = (await replay.until(value => value.kind === 'snapshot')).projection;
  assert.deepEqual(after.runs.map(run => run.startedAt), times);
  assert.deepEqual(after.runs.map(run => run.completedAt), before.runs.map(run => run.completedAt));
  assert.deepEqual(after.turnItems.filter(item => item.type === 'user_message').map(item => item.startedAt), before.turnItems.filter(item => item.type === 'user_message').map(item => item.startedAt));
  assert(after.turnItems.some(item => item.type === 'run_interrupt_result'));
  second.host.stop();
});

test('background history replay preserves the active process mode after earlier completed turns', async () => {
  const engine = new FakeEngine();
  const session = engine.seed('quick-replay-mode', 'Quick view', [
    ['user/message', { id: 'old', source: { kind: 'user' }, content: [{ type: 'text', text: 'old turn' }] }],
    ['turn/end', { turn: 1, reason: { kind: 'completed' } }],
    ['user/message', { id: 'current', source: { kind: 'user' }, content: [{ type: 'text', text: 'current turn' }] }],
    ['turn/start', { turn: 2 }],
  ]);
  Object.assign(session, { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full', runningRuntimeMode: 'plan' });
  const { host } = connect(engine, { throttleMs: 0 });
  await host.start();
  for (let i = 0; i < 100 && host.thread('quick-replay-mode')?.shell().interactionMode !== 'plan'; i++) await tick(5);
  const shell = host.thread('quick-replay-mode').shell();
  assert.equal(shell.runtimeMode, 'approval-required');
  assert.equal(shell.interactionMode, 'plan');
  host.stop();
});

test('background history learns a known binding after an initial access read fails', async () => {
  const engine = new FakeEngine();
  Object.assign(engine.seed('retry-quick-binding', 'Retry binding', [
    ['user/message', { id: 'retry-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'hello' }] }],
    ['turn/end', { reason: { kind: 'completed' } }],
  ]), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full' });
  const request = engine.request.bind(engine); let reads = 0;
  engine.request = (method, params) => method === 'hoh/executor/access/get' && params.sessionId === 'retry-quick-binding' && ++reads === 1
    ? Promise.reject(new Error('not ready')) : request(method, params);
  const { host } = connect(engine, { throttleMs: 0 });
  await host.start();
  for (let i = 0; i < 100 && !host.thread('retry-quick-binding')?.bindingKnown; i++) await tick(5);
  assert.equal(host.thread('retry-quick-binding')?.shell().runtimeMode, 'full-access');
  host.stop();
});

test('journal user message and end survive host restart for streaming, approval and subagent stops', async () => {
  for (const kind of ['stream', 'approval', 'subagent']) {
    const engine = new FakeEngine();
    const id = `stop-${kind}`;
    const session = engine.seed(id, 'Stopped', []);
    const base = 1700000000000;
    const add = (type, data, offset) => { const event = engine.persist(id, type, data); event.time = base + offset; return event; };
    add('turn/start', { turn: 1 }, 1000);
    add('user/message', { id: 'u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Do work' }] }, 2000);
    add('assistant/message', { turn: 1, step: 1, message: { id: 'a', role: 'assistant', content: [{ type: 'text', text: 'I will start.' }] } }, 3000);
    if (kind !== 'stream') {
      const name = kind === 'subagent' ? 'Agent' : 'Bash';
      add('tool/call', { turn: 1, step: 1, callId: 'call', name, arguments: { command: 'sleep 120', description: 'Investigate' } }, 4000);
      if (kind === 'subagent') add('tool/call', { turn: 1, step: 1, callId: 'child', parentCallId: 'call', name: 'WebFetch', arguments: { url: 'https://example.test/' } }, 5000);
      add('approval/asked', { id: 'approval', callId: kind === 'subagent' ? 'child' : 'call' }, 6000);
      add('tool/result', { turn: 1, step: 1, message: { id: 'result', toolCallId: 'call', isError: true, content: [{ type: 'text', text: 'Stopped' }] } }, 35000);
    }
    add('turn/end', { turn: 1, reason: { kind: 'aborted', reason: { kind: 'user' } } }, 35000);
    add('turn/start', { turn: 2 }, 40000);
    add('user/message', { id: 'u2', source: { kind: 'user' }, content: [{ type: 'text', text: 'Continue afterward' }] }, 41000);
    add('assistant/message', { turn: 2, step: 1, message: { id: 'a2', role: 'assistant', content: [{ type: 'text', text: 'Done afterward.' }] } }, 42000);
    add('turn/end', { turn: 2, reason: { kind: 'completed' } }, 43000);
    session.updatedAt = base + 43000;
    const shared = engine.store;
    for (let restart = 0; restart < 2; restart++) {
      const { host, client } = connect(new FakeEngine({ store: shared }), { storage: null });
      const projection = await client.call('orchestration.getThreadProjection', { threadId: id });
      const run = projection.runs[0];
      assert.equal(Date.parse(run.completedAt) - Date.parse(run.startedAt), 33000, `${kind}: journal duration on restart ${restart}`);
      assert.equal(run.status, 'interrupted');
      assert(projection.turnItems.some(item => item.type === 'run_interrupt_result' && item.runId === run.id));
      host.stop();
    }
  }
});

test('Stop during streaming, approval and subagent work replays the user-message timing after host restart', async () => {
  for (const kind of ['stream', 'approval', 'subagent']) {
    const engine = new FakeEngine(); const id = `live-stop-${kind}`;
    engine.seed(id, 'Live stop', []);
    const { client, host } = connect(engine, { throttleMs: 0 });
    const stream = await client.stream('orchestration.subscribeThread', { threadId: id });
    await stream.until(value => value.kind === 'snapshot');
    const start = engine.emitEvent(id, 'turn/start', { turn: 1 });
    engine.emitEvent(id, 'user/message', { id: 'u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Do work' }] });
    engine.emitEvent(id, 'assistant/message', { turn: 1, step: 1, message: { id: 'a', role: 'assistant', content: [{ type: 'text', text: 'I will start.' }] } });
    if (kind !== 'stream') {
      engine.emitEvent(id, 'tool/call', { turn: 1, step: 1, callId: 'call', name: kind === 'subagent' ? 'Agent' : 'Bash', arguments: { command: 'sleep 120', description: 'Investigate' } });
      if (kind === 'subagent') engine.emitEvent(id, 'tool/call', { turn: 1, step: 1, callId: 'child', parentCallId: 'call', name: 'WebFetch', arguments: { url: 'https://example.test/' } });
      const callId = kind === 'subagent' ? 'child' : 'call';
      engine.emitEvent(id, 'approval/asked', { id: 'approval', callId });
      engine.askApproval(id, 'approval', 'Allow?');
    }
    await client.call('orchestration.dispatchCommand', { type: 'thread.stop', commandId: `stop-${kind}`, threadId: id });
    assert.equal(engine.cancelled, 1);
    engine.clock = start.time + 33000;
    engine.emitEvent(id, 'turn/end', { turn: 1, reason: { kind: 'aborted', reason: { kind: 'user' } } });
    const stopped = await stream.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.completedAt === new Date(start.time + 34000).toISOString());
    assert.equal(stopped.projection.runs[0].status, 'interrupted');
    assert.equal(Date.parse(stopped.projection.runs[0].completedAt) - Date.parse(stopped.projection.runs[0].startedAt), 33000, `${kind}: live duration`);
    host.stop();
    const reopened = connect(new FakeEngine({ store: engine.store }), { storage: null });
    const projection = await reopened.client.call('orchestration.getThreadProjection', { threadId: id });
    assert.equal(Date.parse(projection.runs[0].completedAt) - Date.parse(projection.runs[0].startedAt), 33000, kind);
    assert(projection.turnItems.some(item => item.type === 'run_interrupt_result'));
    reopened.host.stop();
  }
});

test('trusted Claude launches are recorded even when the chat is not open', async () => {
  const engine = new FakeEngine();
  Object.assign(engine.seed('background-trust', 'Background', []), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'confirm', cwd: '/home/person/Documents/HoH' });
  const values = new Map();
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
  const { host } = connect(engine, { storage }); await host.start();
  const original = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/access/get' && params.sessionId === 'background-trust') {
      const result = await original(method, params);
      return { ...result, session: { ...result.session, executor: 'claude', running: 'confirm' }, effective: { flags: ['--setting-sources', 'user,project,local'] } };
    }
    if (method === 'hoh/executor/trust/get') return { trusted: true, changed: false, cwd: params.cwd };
    return original(method, params);
  };
  engine.emitEvent('background-trust', 'plugin:external-harness/launch', { runtimeMode: 'confirm', interactionMode: 'default' });
  const key = 'hoh-host.thread-metadata.v1';
  for (let i = 0; i < 100 && !JSON.parse(values.get(key) ?? '{}')['background-trust']?.appliedProjectTrust; i++) await tick(5);
  const saved = JSON.parse(values.get(key))['background-trust'].appliedProjectTrust;
  assert.deepEqual(saved, { cwd: engine.store.get('background-trust').cwd, executor: 'claude' });
  host.stop();
  const restored = connect(engine, { storage }); await restored.host.start();
  assert.deepEqual(JSON.parse(values.get(key))['background-trust'].appliedProjectTrust, saved);
  engine.emitEvent('background-trust', 'plugin:external-harness/folder-trust-changed', {});
  assert.equal(JSON.parse(values.get(key))['background-trust'].appliedProjectTrust, null);
  restored.host.stop();
});

test('between-turn approvals do not become work of the completed turn, while in-turn approvals stay attached', async () => {
  const engine = new FakeEngine();
  engine.seed('between-turns', 'Timing', [
    ['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'first' }] }],
    ['assistant/message', { turn: 1, step: 1, message: { id: 'a1', content: [{ type: 'text', text: 'done' }] } }],
    ['turn/end', { turn: 1, reason: { kind: 'completed' } }],
  ]);
  const { host } = connect(engine);
  await host.start();
  const streamClient = new WireClient(mod.socket.HohWebSocket, new mod.wire.WireServer(host.handlers()));
  const stream = await streamClient.stream('orchestration.subscribeThread', { threadId: 'between-turns' });
  const before = (await stream.until(value => value.kind === 'snapshot' && value.projection.runs.length === 1)).projection;
  const prior = before.runs[0];
  for (const outcome of ['allowed-once', 'rejected', 'cancelled']) {
    const approvalId = `mode-${outcome}`;
    engine.askApproval('between-turns', approvalId, 'Return Claude to Supervised mode?');
    const pending = (await stream.until(value => value.kind === 'snapshot' && value.projection.turnItems.some(item => item.requestId === approvalId))).projection;
    assert.equal(pending.turnItems.find(item => item.requestId === approvalId).runId, null);
    engine.notify('approval.resolved', { approvalId, outcome });
    await tick(20);
    const resolved = host.thread('between-turns').projection();
    assert(!resolved.turnItems.some(item => item.requestId === approvalId));
    assert.deepEqual(resolved.runs[0], prior);
  }
  engine.emitEvent('between-turns', 'user/message', { id: 'u2', source: { kind: 'user' }, content: [{ type: 'text', text: 'second' }] });
  engine.askApproval('between-turns', 'tool-approval', 'Allow this command?');
  await tick(20);
  const active = host.thread('between-turns').projection();
  assert.equal(active.turnItems.find(item => item.requestId === 'tool-approval').runId, active.runs[1].id);
  host.stop(); streamClient.close();
  const restarted = connect(new FakeEngine({ store: engine.store }));
  const reopened = await restarted.client.stream('orchestration.subscribeThread', { threadId: 'between-turns' });
  const restored = (await reopened.until(value => value.kind === 'snapshot' && value.projection.runs.length === 2)).projection;
  assert.deepEqual(restored.runs[0], prior);
  assert(!restored.turnItems.some(item => item.requestId?.startsWith('mode-')));
  restarted.host.stop(); restarted.client.close();
});

test('recovered engine interruption remains distinct from a user stop', async () => {
  const engine = new FakeEngine();
  engine.seed('crashed-run', 'Crash recovery', [
    ['turn/start', { turn: 1 }],
    ['user/message', { id: 'before-crash', source: { kind: 'user' }, content: [{ type: 'text', text: 'Run the task' }] }],
    ['tool/call', { turn: 1, callId: 'crashed-call', name: 'Bash', arguments: '{"command":"sleep 40"}' }],
    ['turn/end', { turn: 1, reason: { kind: 'interrupted' } }],
  ]);
  const { host, client } = connect(engine);
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'crashed-run' });
  const result = (await stream.until(value => value.kind === 'snapshot' && value.projection.runs.length)).projection;
  assert.equal(result.runs[0].status, 'interrupted');
  assert(result.turnItems.some(item => item.type === 'system_notice' && item.message === 'Interrupted'));
  assert(!result.turnItems.some(item => item.type === 'run_interrupt_result'));
  host.stop(); client.close();
});

test('a running Agent or Task call exposes its task description in the timeline', async () => {
  const engine = new FakeEngine();
  engine.seed('agent-description', 'Agent', []);
  const { host, client } = connect(engine);
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'agent-description' });
  await stream.until(value => value.kind === 'snapshot');
  engine.emitEvent('agent-description', 'user/message', { id: 'u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Delegate' }] });
  engine.emitEvent('agent-description', 'tool/call', { turn: 1, callId: 'a', name: 'Agent', arguments: JSON.stringify({ description: 'Count the words', prompt: 'Read notes.txt' }) });
  engine.emitEvent('agent-description', 'tool/call', { turn: 1, callId: 'b', name: 'Task', arguments: JSON.stringify({ prompt: '  Check the repository  ' }) });
  await tick(20);
  const items = host.thread('agent-description').projection().turnItems;
  assert.equal(items.find(item => item.toolName === 'Agent').title, 'Count the words');
  assert.equal(items.find(item => item.toolName === 'Task').title, 'Check the repository');
  host.stop(); client.close();
});

test('one stream attempt can deliver reasoning followed by text without losing the live answer', async () => {
  const engine = new FakeEngine(); engine.seed('mixed-stream', 'Mixed', []);
  const { host, client } = connect(engine);
  const stream = await client.stream('orchestration.subscribeThread', { threadId: 'mixed-stream' });
  await stream.until(value => value.kind === 'snapshot');
  engine.emitEvent('mixed-stream', 'user/message', { id: 'u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Say hello' }] });
  engine.emitFrame('mixed-stream', { type: 'start', attemptId: 'mixed', revision: 1, turn: 1, step: 1 });
  engine.emitFrame('mixed-stream', { type: 'chunk', attemptId: 'mixed', revision: 1, index: 0, chunk: { type: 'reasoning-delta', text: '' } });
  engine.emitFrame('mixed-stream', { type: 'chunk', attemptId: 'mixed', revision: 1, index: 1, chunk: { type: 'reasoning-delta', text: 'Consider' } });
  engine.emitFrame('mixed-stream', { type: 'chunk', attemptId: 'mixed', revision: 1, index: 2, chunk: { type: 'text-delta', text: 'Hello ' } });
  await tick(30);
  let items = host.thread('mixed-stream').projection().turnItems;
  assert(items.some(item => item.type === 'reasoning' && item.streaming));
  assert(items.some(item => item.type === 'assistant_message' && item.streaming && item.text === 'Hello '));
  engine.emitFrame('mixed-stream', { type: 'chunk', attemptId: 'mixed', revision: 1, index: 2, chunk: { type: 'text-delta', text: 'duplicate' } });
  engine.emitFrame('mixed-stream', { type: 'chunk', attemptId: 'mixed', revision: 1, index: 3, chunk: { type: 'text-delta', text: 'world' } });
  await tick(20);
  assert.equal(host.thread('mixed-stream').projection().turnItems.find(item => item.type === 'assistant_message').text, 'Hello world');
  engine.emitFrame('mixed-stream', { type: 'end', attemptId: 'mixed', revision: 1, index: 4, outcome: 'committed' });
  engine.emitFrame('mixed-stream', { type: 'chunk', attemptId: 'mixed', revision: 1, index: 4, chunk: { type: 'text-delta', text: 'late' } });
  await tick(20);
  items = host.thread('mixed-stream').projection().turnItems;
  assert(items.filter(item => item.type === 'reasoning' || item.type === 'assistant_message').every(item => !item.streaming));
  assert.equal(items.find(item => item.type === 'assistant_message').text, 'Hello world');
  engine.emitEvent('mixed-stream', 'assistant/message', { turn: 1, step: 1, message: { id: 'a', content: [{ type: 'reasoning', text: 'Consider' }, { type: 'text', text: 'Hello world' }] } });
  await tick(30);
  items = host.thread('mixed-stream').projection().turnItems;
  assert.equal(items.filter(item => item.type === 'assistant_message').length, 1);
  assert.equal(items.find(item => item.type === 'assistant_message').text, 'Hello world');
  assert.equal(items.filter(item => item.type === 'reasoning').length, 1);
  assert.equal(items.find(item => item.type === 'reasoning').text, 'Consider');
  engine.emitFrame('mixed-stream', { type: 'start', attemptId: 'mixed-gap', revision: 2, turn: 1, step: 2 });
  engine.emitFrame('mixed-stream', { type: 'chunk', attemptId: 'mixed-gap', revision: 2, index: 0, chunk: { type: 'reasoning-delta', text: 'Thinking' } });
  engine.emitFrame('mixed-stream', { type: 'gap', attemptId: 'mixed-gap', revision: 2, index: 1 });
  engine.emitFrame('mixed-stream', { type: 'chunk', attemptId: 'mixed-gap', revision: 2, index: 2, chunk: { type: 'text-delta', text: 'must not appear' } });
  await tick(20);
  assert(!host.thread('mixed-stream').projection().turnItems.some(item => item.text === 'must not appear'));
  engine.emitEvent('mixed-stream', 'user/message', { id: 'u2', source: { kind: 'user' }, content: [{ type: 'text', text: 'Again' }] });
  engine.emitFrame('mixed-stream', { type: 'start', attemptId: 'mixed-empty', revision: 3, turn: 2, step: 1 });
  engine.emitFrame('mixed-stream', { type: 'chunk', attemptId: 'mixed-empty', revision: 3, index: 0, chunk: { type: 'reasoning-delta', text: '' } });
  engine.emitFrame('mixed-stream', { type: 'chunk', attemptId: 'mixed-empty', revision: 3, index: 1, chunk: { type: 'text-delta', text: 'Prefix' } });
  engine.emitEvent('mixed-stream', 'assistant/message', { turn: 2, step: 1, message: { id: 'a2', content: [{ type: 'text', text: 'Prefix final' }] } });
  await tick(20);
  items = host.thread('mixed-stream').projection().turnItems;
  assert.equal(items.filter(item => item.type === 'assistant_message' && item.text === 'Prefix final').length, 1);
  assert(!items.some(item => item.id === 'item-live-mixed-empty-reasoning'));
  host.stop(); client.close();
});

test('project counts include every unarchived chat, including one with no turns after host restart', async () => {
  const store = new Map();
  const engine = new FakeEngine({ store });
  for (const id of ['one', 'two', 'three', 'empty']) engine.seed(id, id, []);
  const first = connect(engine); await first.host.start();
  assert.equal(first.host.home.projectCounts()['hoh-project'], 4);
  const restarted = connect(new FakeEngine({ store })); await restarted.host.start();
  assert.equal(restarted.host.home.projectCounts()['hoh-project'], 4);
  assert.equal(restarted.host.home.entries().length, 0, 'no invented turn or status for an empty chat');
});


test('early reasoning is assigned to the matching run as soon as its user message arrives', () => {
  const t = new mod.threads.ThreadModel({ id: 'early', createdAt: 1000, model: 'opus', mode: 'confirm' });
  t.applyStreamFrame({ type: 'start', attemptId: 'r', turn: 1, step: 1 }, 1100);
  t.applyStreamFrame({ type: 'chunk', attemptId: 'r', index: 0, chunk: { type: 'reasoning-delta', text: '' } }, 1101);
  t.applyEvent({ seq: 1, type: 'turn/start', time: 1102, data: { turn: 1 } });
  t.applyEvent({ seq: 2, type: 'user/message', time: 1103, data: { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'hello' }] } });
  const projection = t.projection();
  assert.equal(projection.turnItems.find(item => item.type === 'reasoning').runId, projection.runs[0].id);
  t.applyEvent({ seq: 3, type: 'turn/end', time: 1200, data: { reason: { kind: 'completed' } } });
  t.applyStreamFrame({ type: 'start', attemptId: 'r2', turn: 2, step: 1 }, 1300);
  t.applyStreamFrame({ type: 'chunk', attemptId: 'r2', index: 0, chunk: { type: 'reasoning-delta', text: '' } }, 1301);
  t.applyEvent({ seq: 4, type: 'turn/start', time: 1302, data: { turn: 2 } });
  t.applyEvent({ seq: 5, type: 'user/message', time: 1303, data: { id: 'u2', source: { kind: 'user' }, content: [{ type: 'text', text: 'next' }] } });
  const next = t.projection();
  assert.equal(next.turnItems.find(item => item.id === 'item-live-r2-reasoning').runId, next.runs[1].id, 'early frame must not remain on the previous run');
});

test('a final interrupted answer replaces the live answer even when turn/end arrives first', () => {
  const t = new mod.threads.ThreadModel({ id: 'stopped', createdAt: 1000, model: 'opus', mode: 'confirm' });
  t.sendLocal('u1', 'go', 1001);
  t.applyStreamFrame({ type: 'start', attemptId: 'text', turn: 1, step: 1 }, 1002);
  t.applyStreamFrame({ type: 'chunk', attemptId: 'text', index: 0, chunk: { type: 'text-delta', text: '1...136' } }, 1003);
  t.applyEvent({ seq: 1, type: 'turn/end', time: 1004, data: { reason: { kind: 'aborted', reason: { kind: 'user' } } } });
  t.applyEvent({ seq: 2, type: 'assistant/message', time: 1005, data: { turn: 1, step: 1, message: { id: 'final', content: [{ type: 'text', text: '1...137' }] } } });
  assert.deepEqual(t.projection().turnItems.filter(item => item.type === 'assistant_message').map(item => item.text), ['1...137']);
});

test('folder trust session requests wait for resume before accessing a cached session', async () => {
  const engine = new FakeEngine(); engine.seed('saved', 'saved', []);
  const original = engine.request.bind(engine); let release; const gate = new Promise(resolve => { release = resolve; });
  engine.request = async (method, params) => { if (method === 'session/resume') await gate; return original(method, params); };
  const { host } = connect(engine); await host.start();
  const request = host.trustRequest('session/working-directory/get', { sessionId: 'saved' });
  const access = host.trustRequest('hoh/executor/access/get', { sessionId: 'saved', executor: 'claude', trace: 'trust-request' });
  await tick(5);
  assert.equal(engine.requests.some(r => r.method === 'session/working-directory/get' && r.params.sessionId === 'saved'), false);
  assert.equal(engine.requests.some(r => r.method === 'hoh/executor/access/get' && r.params.trace === 'trust-request'), false);
  release(); await Promise.all([request, access]);
  const methods = engine.requests.filter(r => r.params?.sessionId === 'saved').map(r => r.method);
  assert(methods.indexOf('session/resume') < methods.indexOf('session/working-directory/get'));
  assert(engine.requests.findIndex(r => r.method === 'session/resume' && r.params.sessionId === 'saved') < engine.requests.findIndex(r => r.params?.trace === 'trust-request'));
});

test('a live session joins its project as soon as the engine reports its folder', async () => {
  const engine = new FakeEngine(); const { host, client } = connect(engine); await host.start();
  await client.call('projects.mutate', { type: 'project.create', commandId: 'p', projectId: 'p-live', workspaceRoot: '/home/person/Projects', title: 'Projects' });
  engine.seed('new-live', 'new-live', []);
  engine.emitEvent('new-live', 'working-directory/change', { cwd: '/home/person/Projects' });
  host.flush();
  assert.equal(host.home.projectCounts()['p-live'], 1);
});

test('a failed session resume never falls through to a folder lookup that could create the session', async () => {
  const engine = new FakeEngine(); engine.seed('resume-fails', 'saved', []);
  const original = engine.request.bind(engine); let blocked = true;
  engine.request = async (method, params) => { if (blocked && method === 'session/resume' && params.sessionId === 'resume-fails') throw new Error('resume refused'); return original(method, params); };
  const { host } = connect(engine); await host.start();
  await assert.rejects(host.trustRequest('session/working-directory/get', { sessionId: 'resume-fails' }));
  assert.equal(engine.requests.some(r => r.method === 'session/working-directory/get' && r.params.sessionId === 'resume-fails'), false);
  blocked = false; await host.trustRequest('session/working-directory/get', { sessionId: 'resume-fails' });
  const methods = engine.requests.filter(r => r.params?.sessionId === 'resume-fails').map(r => r.method);
  assert(methods.indexOf('session/resume') < methods.indexOf('session/working-directory/get'));
});

test('an SDK session already open or opening remains usable after restart', async () => {
  const engine = new FakeEngine(); engine.seed('open-session', 'saved', []);
  const original = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'session/resume' && params.sessionId === 'open-session') throw new Error('SDK session is already open or opening: open-session');
    return original(method, params);
  };
  const { host } = connect(engine); await host.start();
  const folder = await host.trustRequest('session/working-directory/get', { sessionId: 'open-session' });
  assert.equal(folder.cwd, '/home/person/Documents/HoH');
});

test('user stop time survives a later final event and a history replay', () => {
  const make = () => new mod.threads.ThreadModel({ id: 'c11-stop', createdAt: 1000, model: 'opus', mode: 'confirm' });
  const events = [
    { seq: 1, type: 'turn/start', time: 1100, data: { turn: 1 } },
    { seq: 2, type: 'user/message', time: 1200, data: { id: 'u', source: { kind: 'user' }, content: [{ type: 'text', text: 'count' }] } },
    { seq: 3, type: 'assistant/message', time: 5900, data: { turn: 1, step: 1, message: { id: 'a', content: [{ type: 'text', text: 'answer' }] } } },
    { seq: 4, type: 'turn/end', time: 6000, data: { reason: { kind: 'aborted', reason: { kind: 'user' } } } },
  ];
  const t=make(); t.sendLocal('local','count',1000); t.applyEvent(events[0]); t.markStopTime(5000); t.interrupt(5000); t.applyEvent(events[2]); t.applyEvent(events[3]);
  assert.equal(Date.parse(t.projection().runs[0].completedAt),5000);
  const replay=make(); replay.restoreStopTimes({ '1': 5000 }); for(const event of events) replay.applyEvent(event);
  assert.equal(Date.parse(replay.projection().runs[0].completedAt),5000);
});

test('Codex command approval shows the command once and restores the work item after approval', () => {
  const t = new mod.threads.ThreadModel({ id:'codex-approval',createdAt:1000,model:'gpt-6.2-sol',mode:'confirm',executor:'codex' });
  t.sendLocal('u','go',1001);
  t.applyEvent({seq:1,type:'tool/call',time:1002,data:{turn:1,callId:'cmd',name:'bash',arguments:{command:'echo hi'}}});
  t.applyEvent({seq:2,type:'approval/asked',time:1003,data:{id:'ask',callId:'cmd',toolName:'bash'}});
  t.approvalPending({approvalId:'ask',humanText:'Run an action that cannot be checked in advance'},1004);
  const pending=t.projection();
  assert.equal(pending.turnItems.find(item=>item.type==='approval_request').prompt,'echo hi');
  assert.equal(pending.turnItems.filter(item=>item.type==='command_execution').length,0);
  t.approvalResolved({approvalId:'ask',outcome:'allowed-once'},1005);
  assert.equal(t.projection().turnItems.filter(item=>item.type==='command_execution').length,1);
});

test('Stop timestamp remains fixed across delayed engine end and a new host', async () => {
  const engine = new FakeEngine(); const id = 'stop-click-time';
  engine.seed(id, 'Stop time', []);
  const values = new Map();
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
  const first = connect(engine, { storage, now: () => engine.clock });
  const stream = await first.client.stream('orchestration.subscribeThread', { threadId: id });
  await stream.until(value => value.kind === 'snapshot');
  engine.emitEvent(id, 'turn/start', { turn: 1 });
  engine.emitEvent(id, 'user/message', { id: 'u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Count' }] });
  engine.clock += 4300;
  const stopAt = engine.clock;
  await first.client.call('orchestration.dispatchCommand', { type: 'thread.stop', commandId: 'stop-click', threadId: id });
  engine.clock += 800;
  engine.emitEvent(id, 'assistant/message', { turn: 1, step: 1, message: { id: 'a', role: 'assistant', content: [{ type: 'text', text: 'partial' }] } });
  engine.emitEvent(id, 'turn/end', { turn: 1, reason: { kind: 'aborted', reason: { kind: 'user' } } });
  const live = await first.client.call('orchestration.getThreadProjection', { threadId: id });
  assert.equal(Date.parse(live.runs[0].completedAt), stopAt);
  first.host.stop();
  const second = connect(new FakeEngine({ store: engine.store }), { storage, now: () => engine.clock });
  const restored = await second.client.call('orchestration.getThreadProjection', { threadId: id });
  assert.equal(restored.runs[0].completedAt, live.runs[0].completedAt);
  assert.equal(restored.runs[0].startedAt, live.runs[0].startedAt);
  second.host.stop();
});

test('Codex approval obtains its actual command even when the native request arrives first', () => {
  const t = new mod.threads.ThreadModel({ id:'codex-reordered', createdAt:1000, model:'gpt-6.2-sol', mode:'confirm', executor:'codex' });
  t.sendLocal('u', 'go', 1001);
  t.applyEvent({ seq:1, type:'tool/call', time:1002, data:{turn:1,callId:'cmd',name:'bash',arguments:JSON.stringify({command:"/usr/bin/zsh -c 'echo hi > file'"})} });
  t.approvalPending({approvalId:'ask',humanText:'generic'},1003);
  t.applyEvent({seq:2,type:'approval/asked',time:1004,data:{id:'ask',callId:'cmd',toolName:'bash'}});
  const pending=t.projection();
  assert.equal(pending.turnItems.find(item=>item.type==='approval_request').prompt,"/usr/bin/zsh -c 'echo hi > file'");
  assert.equal(pending.turnItems.filter(item=>item.type==='command_execution').length,0);
});

test('a reasoning frame for the next turn never belongs to the previous completed run', () => {
  const t = new mod.threads.ThreadModel({ id:'early-next', createdAt:1000, model:'opus', mode:'confirm' });
  t.sendLocal('old','old message',1001);
  t.applyEvent({seq:1,type:'turn/end',time:2000,data:{turn:1,reason:{kind:'completed'}}});
  t.applyStreamFrame({type:'start',attemptId:'early',turn:2,step:1},3000);
  t.applyStreamFrame({type:'chunk',attemptId:'early',index:0,chunk:{type:'reasoning-delta',text:''}},3001);
  assert.equal(t.projection().turnItems.find(item=>item.type==='reasoning').runId,null);
  t.sendLocal('new','new message',3002);
  assert.equal(t.projection().turnItems.find(item=>item.type==='reasoning').runId,t.projection().runs[1].id);
});

test('late tool result cannot move a stopped run timeline beyond the Stop click', () => {
  const t = new mod.threads.ThreadModel({id:'late-stop-tool',createdAt:1000,model:'opus',mode:'confirm'});
  t.sendLocal('u','run',1100);
  t.applyEvent({seq:1,type:'tool/call',time:1200,data:{turn:1,callId:'c',name:'bash',arguments:{command:'sleep 1'}}});
  t.markStopTime(5000); t.interrupt(5000);
  t.applyEvent({seq:2,type:'tool/result',time:5800,data:{turn:1,message:{toolCallId:'c',content:[{type:'text',text:'stopped'}]}}});
  const item=t.projection().turnItems.find(item=>item.type==='command_execution');
  assert.equal(Date.parse(item.updatedAt),5000);
  assert.equal(Date.parse(item.completedAt),5000);
  const replay = new mod.threads.ThreadModel({id:'late-stop-tool',createdAt:1000,model:'opus',mode:'confirm'});
  replay.restoreStopTimes({'1':5000});
  replay.applyEvent({seq:1,type:'user/message',time:1100,data:{id:'u',source:{kind:'user'},content:[{type:'text',text:'run'}]}});
  replay.applyEvent({seq:2,type:'tool/call',time:1200,data:{turn:1,callId:'c',name:'bash',arguments:{command:'sleep 1'}}});
  replay.applyEvent({seq:3,type:'tool/result',time:5800,data:{turn:1,message:{toolCallId:'c',content:[{type:'text',text:'stopped'}]}}});
  replay.applyEvent({seq:4,type:'turn/end',time:5801,data:{turn:1,reason:{kind:'aborted',reason:{kind:'user'}}}});
  const replayed = replay.projection().turnItems.find(item=>item.type==='command_execution');
  assert.equal(replayed.updatedAt,item.updatedAt);
  assert.equal(replayed.completedAt,item.completedAt);
});

test('a live engine turn counter that differs from the UI run ordinal keeps its active run', () => {
  const t = new mod.threads.ThreadModel({id:'engine-turn-counter',createdAt:1000,model:'opus',mode:'confirm'});
  t.sendLocal('u','go',1001);
  t.applyStreamFrame({type:'start',attemptId:'stream',turn:9,step:1},1002);
  t.applyStreamFrame({type:'chunk',attemptId:'stream',index:0,chunk:{type:'text-delta',text:'partial'}},1003);
  assert.equal(t.projection().turnItems.find(item=>item.type==='assistant_message').runId,t.projection().runs[0].id);
});
