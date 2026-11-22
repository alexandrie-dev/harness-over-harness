// The host against T3's real client stack: effect's RpcClient.make(WsRpcGroup) with
// makeProtocolSocket + RpcSerialization.json over the host's in-memory WebSocket, exactly as
// packages/client-runtime/src/rpc/session.ts builds it. Every answer is decoded by T3's own schemas.
import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { rm } from 'node:fs/promises';
import * as Effect from 'effect/Effect';
import * as Layer from 'effect/Layer';
import * as Stream from 'effect/Stream';
import * as Fiber from 'effect/Fiber';
import * as Exit from 'effect/Exit';
import * as Cause from 'effect/Cause';
import * as DateTime from 'effect/DateTime';
import * as Socket from 'effect/socket/Socket';
import { RpcClient, RpcSerialization } from 'effect/rpc';
import { buildHost } from './build.mjs';
import { buildContracts } from './contracts.mjs';
import { FakeEngine } from './fake-engine.mjs';

let built; let contracts; let mod; let WsRpcGroup;
before(async () => {
  built = await buildHost();
  contracts = await buildContracts();
  WsRpcGroup = (await contracts.load('index')).WsRpcGroup;
  mod = { host: await built.load('host'), wire: await built.load('wire'), socket: await built.load('socket') };
});
after(async () => {
  await rm(built.dir, { recursive: true, force: true });
  await rm(contracts.dir, { recursive: true, force: true });
});

/** Runs `body(client, engine)` with a real T3 RPC client connected to a host over `engine`. */
function withClient(engine, body) {
  const host = new mod.host.HohHost({ engine, throttleMs: 0 });
  const server = new mod.wire.WireServer(host.handlers(), { onError: () => {} });
  const layerProtocol = Layer.effect(RpcClient.Protocol, RpcClient.makeProtocolSocket({ pingTimeout: '15 seconds', retryTransientErrors: false })).pipe(
    Layer.provide(Layer.mergeAll(
      Socket.layerWebSocket('ws://hoh.local/ws?frameParts=1', { openTimeout: '5 seconds' }).pipe(
        Layer.provide(Layer.succeed(Socket.WebSocketConstructor, url => new mod.socket.HohWebSocket(url, server))),
      ),
      RpcSerialization.layerJson,
    )),
  );
  return Effect.runPromise(Effect.scoped(Effect.gen(function* () {
    const context = yield* Layer.build(layerProtocol);
    const client = yield* RpcClient.make(WsRpcGroup).pipe(Effect.provide(context));
    return yield* Effect.promise(() => body(client, engine, host));
  })));
}
const run = effect => Effect.runPromise(effect);
const runExit = effect => Effect.runPromise(Effect.exit(effect));
/** Collects items of a stream in the background; `until` waits for a decoded item that matches. */
async function collect(stream) {
  const items = [];
  const fiber = Effect.runFork(Stream.runForEach(stream, item => Effect.sync(() => { items.push(item); })));
  return {
    items,
    async until(match, ms = 3000) {
      const deadline = Date.now() + ms;
      for (;;) {
        const found = items.find(match);
        if (found) return found;
        const exit = fiber.pollUnsafe?.();
        if (exit && Exit.isFailure(exit)) throw new Error('stream failed: ' + Cause.pretty(exit.cause));
        if (Date.now() > deadline) throw new Error('no matching item; last: ' + JSON.stringify(items.slice(-2)).slice(0, 800));
        await new Promise(resolve => setTimeout(resolve, 10));
      }
    },
    stop: () => Effect.runPromise(Fiber.interrupt(fiber)),
  };
}

test('T3 client: handshake decodes (config stream, getConfig, probe, settings, updateSettings)', () => withClient(new FakeEngine(), async client => {
  const config = await collect(client['subscribeServerConfig']({ environmentThemes: true, usageLimitSources: true, usageLimitsCommand: true, workspaceSnapshotsOnDemand: true }));
  const snapshot = await config.until(event => event.type === 'snapshot');
  assert.equal(snapshot.config.environment.environmentId, 'hoh-local');
  assert.equal(snapshot.config.providers[0].instanceId, 'claudeAgent');
  assert.equal(snapshot.config.settings.defaultRuntimeMode, 'approval-required');
  assert.equal(snapshot.config.settings.responseStreamingMode, 'paragraph', 'defaults filled in by T3\'s schema');
  await config.stop();
  assert.deepEqual(await run(client['server.probe']({})), {});
  await run(client['server.reportClientActivity']({ clientId: 'window-1', clientKind: 'web', visible: true, focused: true, recentlyInteracted: true, scopes: [], observedAt: DateTime.makeUnsafe('2026-11-22T19:00:00.000Z') }));
  const direct = await run(client['server.getConfig']({}));
  assert.equal(direct.environment.orchestrationProtocolVersion, 2);
  const settings = await run(client['server.getSettings']({}));
  assert.deepEqual({ ...settings.defaultModelSelection }, { instanceId: 'claudeAgent', model: 'claude-sonnet' });
  const updated = await run(client['server.updateSettings']({ patch: { responseStreamingMode: 'turn' } }));
  assert.equal(updated.responseStreamingMode, 'turn');
  const providers = await run(client['server.refreshProviders']({}));
  assert.equal(providers.providers[0].models.length, 2);
}));

test('T3 client: unsupported methods fail with the typed error; hidden streams stay quiet', () => withClient(new FakeEngine(), async client => {
  const exit = await runExit(client['projects.searchEntries']({ cwd: '/x', query: 'a', limit: 5 }));
  assert(Exit.isFailure(exit));
  const error = Cause.squash(exit.cause);
  assert.equal(error._tag, 'EnvironmentAuthorizationError');
  assert.equal(error.message, 'Not available in HoH.');
}));

test('T3 client: list, new chat, streamed answer, tool step, permission, question, Stop — all decode', () => {
  const engine = new FakeEngine();
  engine.seed('chat-old', 'Старый', [['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'Привет' }] }], ['turn/start', {}], ['assistant/message', { turn: 1, step: 1, message: { id: 'a1', content: [{ type: 'text', text: 'Здравствуйте' }] } }], ['turn/end', { reason: { kind: 'completed' } }]]);
  return withClient(engine, async client => {
    const shell = await collect(client['orchestration.subscribeShell']({ requestCompletionMarker: true }));
    const first = await shell.until(item => item.kind === 'snapshot');
    assert.deepEqual(first.snapshot.threads.map(thread => thread.id), ['chat-old']);
    assert(DateTime.isDateTime(first.snapshot.threads[0].createdAt));
    await shell.until(item => item.kind === 'synchronized');

    const old = await run(client['orchestration.getThreadProjection']({ threadId: 'chat-old' }));
    assert.deepEqual(old.turnItems.map(item => item.type), ['user_message', 'assistant_message']);
    assert.equal(old.visibleTurnItems.length, 2);

    const launched = await run(client['orchestration.launchThread']({
      commandId: 'cmd-1', threadId: 'thread-1', projectId: 'hoh-project', title: 'Hello', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
      runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' },
      initialMessage: { messageId: 'm-1', text: 'Сделай', attachments: [] },
    }));
    assert.equal(launched.threadId, 'thread-1');
    assert.equal(launched.projection.runs[0].status, 'running');

    const thread = await collect(client['orchestration.subscribeThread']({ threadId: 'thread-1', requestCompletionMarker: true, acceptBoundedSnapshot: true, acceptCompactIds: true }));
    await thread.until(item => item.kind === 'synchronized');
    const items = () => thread.items.filter(item => item.kind === 'snapshot').at(-1).projection.turnItems;

    engine.emitFrame('thread-1', { type: 'start', attemptId: 'x1', revision: 1, turn: 1, step: 1 });
    engine.emitFrame('thread-1', { type: 'chunk', attemptId: 'x1', revision: 1, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Работаю' } });
    await thread.until(item => item.kind === 'snapshot' && item.projection.turnItems.some(entry => entry.type === 'assistant_message' && entry.streaming));
    engine.emitEvent('thread-1', 'tool/call', { turn: 1, callId: 'c1', name: 'Write', arguments: { file_path: '/home/person/Documents/HoH/a.txt', content: 'hi' } });
    engine.emitEvent('thread-1', 'tool/call', { turn: 1, callId: 'c2', name: 'Read', arguments: { file_path: 'b.txt' } });
    engine.emitEvent('thread-1', 'tool/call', { turn: 1, callId: 'c3', name: 'Grep', arguments: { pattern: 'x' } });
    engine.emitEvent('thread-1', 'tool/call', { turn: 1, callId: 'c4', name: 'WebSearch', arguments: { query: 'q' } });
    engine.emitEvent('thread-1', 'tool/call', { turn: 1, callId: 'c5', name: 'Bash', arguments: { command: 'false' } });
    engine.emitEvent('thread-1', 'tool/result', { turn: 1, message: { toolCallId: 'c5', isError: true, content: [{ type: 'text', text: 'boom [Command finished with exit code 1]' }] } });
    await thread.until(item => item.kind === 'snapshot' && item.projection.turnItems.some(entry => entry.type === 'command_execution' && entry.status === 'failed'));
    assert.deepEqual(items().map(item => item.type), ['user_message', 'assistant_message', 'file_change', 'dynamic_tool', 'file_search', 'web_search', 'command_execution']);
    assert.equal(items().at(-1).exitCode, 1);

    engine.askApproval('thread-1', 'appr-1', 'Claude wants to run: rm x');
    const asked = await thread.until(item => item.kind === 'snapshot' && item.projection.runtimeRequests.length === 1);
    assert.equal(asked.projection.runtimeRequests[0].responseCapability.type, 'live');
    const result = await run(client['orchestration.dispatchCommand']({ type: 'runtime-request.respond', commandId: 'c-1', threadId: 'thread-1', requestId: 'appr-1', decision: 'accept' }));
    assert(Number.isInteger(result.sequence));
    assert.deepEqual(engine.decisions, [{ approvalId: 'appr-1', outcome: 'allowed-once' }]);

    engine.askQuestion({ requestId: 'q-1', sessionId: 'thread-1', questions: [{ id: 'pick', header: 'Pick', question: 'Which?', options: [{ label: 'A' }, { label: 'B', description: 'second' }], multiSelect: true }] });
    await thread.until(item => item.kind === 'snapshot' && item.projection.turnItems.some(entry => entry.type === 'user_input_request'));
    await run(client['orchestration.dispatchCommand']({ type: 'runtime-request.respond', commandId: 'c-2', threadId: 'thread-1', requestId: 'q-1', answers: { pick: ['A', 'B'] } }));
    assert.deepEqual(engine.answers.at(-1).answer, { answers: [{ id: 'pick', selected: ['A', 'B'] }] });

    await run(client['orchestration.dispatchCommand']({ type: 'thread.stop', commandId: 'c-3', threadId: 'thread-1' }));
    assert.equal(engine.cancelled, 1);
    engine.emitEvent('thread-1', 'turn/end', { reason: { kind: 'aborted' } });
    const stopped = await thread.until(item => item.kind === 'snapshot' && item.projection.runs[0].status === 'interrupted');
    assert(stopped.projection.turnItems.every(item => item.status !== 'running'));
    await shell.until(item => item.kind === 'thread.updated' && item.thread.id === 'thread-1' && item.thread.status === 'interrupted');

    const titles = await run(client['orchestration.getThreadTitles']({ threadIds: ['thread-1', 'nope'] }));
    assert.deepEqual({ ...titles }, { 'thread-1': 'Hello' });
    const force = await run(client['orchestration.forceStopThread']({ threadId: 'thread-1' }));
    assert.equal(force.outcome, 'killed');
    await run(client['orchestration.dispatchCommand']({ type: 'thread.runtime-mode.set', commandId: 'c-4', threadId: 'thread-1', runtimeMode: 'full-access' }));
    await run(client['orchestration.dispatchCommand']({ type: 'thread.model-selection.set', commandId: 'c-5', threadId: 'thread-1', modelSelection: { instanceId: 'claudeAgent', model: 'claude-opus' } }));
    const exit = await runExit(client['orchestration.dispatchCommand']({ type: 'thread.delete', commandId: 'c-6', threadId: 'thread-1' }));
    assert(Exit.isFailure(exit));
    const error = Cause.squash(exit.cause);
    assert.deepEqual([error._tag, error.commandType, error.message], ['OrchestrationV2DispatchCommandError', 'thread.delete', 'Not available in HoH.']);
    await thread.stop(); await shell.stop();
  });
});

test('T3 schemas: the HTTP answers before the socket decode (session state, environment descriptor)', async () => {
  const Schema = await import('effect/Schema');
  const index = await contracts.load('index');
  const http = await built.load('http');
  const session = Schema.decodeUnknownSync(index.AuthSessionState)(await http.hohHttpResponse('GET', '/api/auth/session').json());
  assert.equal(session.authenticated, true);
  const descriptor = Schema.decodeUnknownSync(index.ExecutionEnvironmentDescriptor)(await http.hohHttpResponse('GET', '/.well-known/t3/environment').json());
  assert.equal(descriptor.environmentId, 'hoh-local');
});

test('T3 client: the welcome that opens the app, and a folder that is not a git repository — decode', () => withClient(new FakeEngine(), async client => {
  const lifecycle = await collect(client['subscribeServerLifecycle']({}));
  const welcome = await lifecycle.until(event => event.type === 'welcome');
  assert.equal(welcome.payload.environment.environmentId, 'hoh-local');
  assert.equal(welcome.payload.bootstrapThreadId, undefined);
  await lifecycle.stop();
  const vcs = await collect(client['subscribeVcsStatus']({ cwd: '/home/person/Documents/HoH' }));
  const status = await vcs.until(event => event._tag === 'snapshot');
  assert.equal(status.local.isRepo, false);
  await vcs.stop();
  assert.equal((await run(client['vcs.refreshStatus']({ cwd: '/x' }))).isRepo, false);
  assert.equal((await run(client['vcs.listRefs']({ cwd: '/x' }))).isRepo, false);
}));
