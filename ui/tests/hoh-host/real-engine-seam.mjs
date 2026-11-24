// Production HohHost driven by the Rust desktop seam over JSON lines. Rust owns the real engine,
// main-window confirmation and approval Broker; this file owns exactly the T3 host/client side.
import assert from 'node:assert/strict';
import { createInterface } from 'node:readline';
import { rm } from 'node:fs/promises';
import { buildHost } from './build.mjs';
import { WireClient } from './wire-client.mjs';

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const write = value => process.stdout.write(JSON.stringify(value) + '\n');

class NativeEngine {
  constructor() {
    this.next = 0;
    this.pending = new Map();
    this.listeners = new Set();
    this.statusListeners = new Set();
    this.input = createInterface({ input: process.stdin });
    this.input.on('line', line => this.receive(JSON.parse(line)));
  }
  receive(frame) {
    if (frame.type === 'response') {
      const pending = this.pending.get(frame.id);
      if (!pending) return;
      this.pending.delete(frame.id);
      frame.error ? pending.reject(frame.error) : pending.resolve(frame.result);
      return;
    }
    if (frame.type === 'notification') for (const listener of this.listeners) listener({ method: frame.method, params: frame.params });
    if (frame.type === 'status') for (const listener of this.statusListeners) listener(frame.status);
  }
  call(type, body) {
    const id = String(++this.next);
    write({ type, id, ...body });
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }
  async request(method, params) {
    const trace = this.trace && process.env.HOH_SEAM_TRACE && ['session/list', 'session/resume', 'session/events', 'hoh/executor/access/get'].includes(method);
    if (trace) console.error('→', method, params?.sessionId ?? '');
    try {
      const result = await this.call('request', { method, params });
      if (trace) console.error('←', method, JSON.stringify(result?.session ?? { events: result?.events?.length }));
      return result;
    } catch (error) {
      if (trace) console.error('←', method, error?.message ?? error);
      throw error;
    }
  }
  decideApproval(approvalId, outcome) { return this.call('decide', { approvalId, outcome }); }
  async subscribe(listener) { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  async subscribeStatus(listener) { this.statusListeners.add(listener); return () => this.statusListeners.delete(listener); }
  close() { this.input.close(); }
}

let built;
let engine;
try {
  built = await buildHost();
  const mod = {
    host: await built.load('host'), wire: await built.load('wire'), socket: await built.load('socket'),
  };
  engine = new NativeEngine();
  const saved = new Map();
  const storage = { getItem: key => saved.get(key) ?? null, setItem: (key, value) => saved.set(key, value) };
  const connect = () => {
    const host = new mod.host.HohHost({ engine, throttleMs: 0, loadRetryMs: 100, loadRetries: 600, storage });
    return { host, client: new WireClient(mod.socket.HohWebSocket, new mod.wire.WireServer(host.handlers())) };
  };
  const { host, client } = connect();
  const config = await client.stream('subscribeServerConfig', {});
  const readyConfig = await config.until(value => value.type === 'snapshot' && value.config.providers[0].supportedRuntimeModes.length === 4, 0, 60_000);
  const model = readyConfig.config.providers[0].models[0]?.slug;
  assert(model, 'the real engine config must offer at least one Claude model');

  const threadId = 'real-seam';
  const thread = await client.stream('orchestration.subscribeThread', { threadId });
  const launched = await client.call('orchestration.launchThread', {
    commandId: 'launch-real-seam', threadId, projectId: 'hoh-project', title: 'Real seam',
    modelSelection: { instanceId: 'claudeAgent', model }, runtimeMode: 'auto',
    interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'message-real-seam', text: 'run', attachments: [] },
  });
  assert(['unknown', 'auto'].includes(launched.projection.thread.runtimeMode));
  await thread.until(value => value.kind === 'snapshot' && value.projection.thread.runtimeMode === 'auto', 0, 60_000);

  const cardFor = (value, id) => value.projection.turnItems.find(item => item.type === 'approval_request' && item.requestId === id);
  // Launch can briefly show the approval for changing access mode before the Bash request.
  const pendingRequest = value => value?.kind === 'snapshot'
    ? value.projection.runtimeRequests.find(candidate => candidate.status === 'pending'
      && cardFor(value, candidate.id)?.prompt?.includes('echo real-seam'))
    : undefined;
  let pending = await thread.until(value => pendingRequest(value), 0, 60_000);
  let request = pendingRequest(pending);
  let card = cardFor(pending, request.id);
  let answered = false;
  for (let attempt = 0; attempt < 4 && !answered; attempt++) {
    assert(card, 'the native pending request must become the production T3 card');
    assert.equal(typeof card.prompt, 'string');
    assert.match(card.prompt, /^Claude asks to run:/);
    assert.equal(card.requestId, request.id);

    // The native Broker deliberately refuses an instant allow before the person could read this
    // exact card. A harness retry can replace a cancelled permission id; in that case the person
    // sees the replacement card and its own read delay starts afresh.
    await new Promise(resolve => setTimeout(resolve, 1_100));
    try {
      await client.call('orchestration.dispatchCommand', {
        type: 'runtime-request.respond', commandId: `approve-real-seam-${attempt}`, threadId,
        requestId: request.id, decision: 'accept',
      });
      answered = true;
    } catch (error) {
      const latest = thread.values.at(-1);
      if (pendingRequest(latest) && pendingRequest(latest).id !== request.id) pending = latest;
      else pending = await thread.until(value => pendingRequest(value) && pendingRequest(value).id !== request.id, thread.values.length, 5_000);
      const replacement = pendingRequest(pending);
      if (!replacement || replacement.id === request.id) throw error;
      request = replacement;
      card = cardFor(pending, request.id);
    }
  }
  assert(answered, 'a stable native permission request must be answerable');
  const resolved = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.some(candidate => candidate.id === request.id && candidate.status === 'resolved'), 0, 60_000);
  assert.equal(resolved.projection.runtimeRequests.find(candidate => candidate.id === request.id).decision, 'accept');
  const access = await engine.request('hoh/executor/access/get', { executor: 'claude', sessionId: threadId });
  assert.equal(access.mode, 'confirm', 'the legacy projection must stay downgrade-safe');
  assert.equal(access.session?.runtimeMode, 'auto');

  const send = (rpc, name) => rpc.call('orchestration.dispatchCommand', {
    type: 'message.dispatch', commandId: name, threadId, messageId: name, text: name, attachments: [],
  });
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'completed', 0, 15_000);
  const second = thread.values.length;
  await send(client, 'stop-this-turn');
  await thread.until(value => pendingRequest(value), second, 15_000);
  await pause(1100);
  const beforeStop = thread.values.length;
  await client.call('orchestration.dispatchCommand', { type: 'thread.stop', commandId: 'stop-real-seam', threadId });
  const stopped = value => value.kind === 'snapshot' && value.projection.turnItems.some(item => item.type === 'run_interrupt_result');
  const interrupted = await thread.until(stopped, beforeStop, 15_000);
  const interruptedRun = interrupted.projection.runs.find(run => run.status === 'interrupted');
  assert(interruptedRun && Date.parse(interruptedRun.completedAt) - Date.parse(interruptedRun.startedAt) >= 1_000);
  host.stop();
  client.close();
  engine.trace = true;
  await engine.call('restart', {});
  const cold = connect();
  const coldShell = await cold.client.stream('orchestration.subscribeShell', {});
  await coldShell.until(row => row.kind === 'snapshot' && row.snapshot.threads.some(item => item.id === threadId), 0, 15_000);
  const reopened = await cold.client.stream('orchestration.subscribeThread', { threadId, requestCompletionMarker: true });
  await reopened.until(value => value.kind === 'synchronized' || reopened.exit !== undefined, 0, 15_000);
  assert(!reopened.exit, JSON.stringify(reopened.exit));
  const history = await reopened.until(value => stopped(value) && value.projection.thread.providerInstanceId === 'claudeAgent' && value.projection.thread.runtimeMode === 'auto', 0, 15_000);
  assert(history.projection.messages.some(message => message.text === 'seam complete'));
  assert.equal(history.projection.providerTurns[0]?.tokenUsage?.inputTokens, 40, 'context usage survives restart');
  const restoredStop = history.projection.runs.find(run => run.status === 'interrupted');
  assert.equal(restoredStop?.ordinal, interruptedRun.ordinal);
  const duration = run => Date.parse(run.completedAt) - Date.parse(run.startedAt);
  assert(Math.abs(duration(restoredStop) - duration(interruptedRun)) <= 100, 'stopped duration survives restart');
  const afterRestart = reopened.values.length;
  await cold.client.call('orchestration.dispatchCommand', {
    type: 'thread.runtime-mode.set', commandId: 'supervised-after-restart', threadId, runtimeMode: 'approval-required',
  });
  await send(cold.client, 'subagent-permissions');
  const pendingIds = value => value?.kind === 'snapshot'
    ? value.projection.runtimeRequests.filter(request => request.status === 'pending').map(request => request.id)
    : [];
  const parallel = await reopened.until(value => pendingIds(value).length === 2, afterRestart, 15_000);
  const ids = pendingIds(parallel);
  const agent = parallel.projection.turnItems.find(item => item.toolName === 'Agent');
  assert(agent, 'the native Agent call is visible');
  assert.equal(parallel.projection.turnItems.filter(item => item.type === 'file_change' && item.parentItemId === agent.id).length, 2);
  for (const id of ids) {
    const item = cardFor(parallel, id);
    assert(item && item.parentItemId === null);
    assert.match(item.prompt, /^Claude asks to create file /);
    assert.deepEqual(item.options.map(option => option.label), ['Decline', 'Approve']);
  }
  // The live regression denied after approximately three seconds. Delay longer than that.
  await pause(3_500);
  assert.deepEqual(pendingIds(reopened.values.at(-1)), ids);
  for (const [file, decision] of [['sub-second.txt', 'accept'], ['sub-first.txt', 'decline']]) {
    const id = ids.find(id => cardFor(parallel, id).prompt.includes(file));
    assert(id, file);
    await cold.client.call('orchestration.dispatchCommand', {
      type: 'runtime-request.respond', commandId: `answer-${file}`, threadId, requestId: id, decision,
    });
  }
  await reopened.until(value => value.kind === 'snapshot' && value.projection.runs.at(-1)?.status === 'completed', afterRestart, 15_000);
  cold.host.stop();
  cold.client.close();
  write({ type: 'result', ok: true, approvalId: request.id, humanText: card.prompt, mode: access.session.runtimeMode });
} catch (error) {
  write({ type: 'result', ok: false, error: error?.stack ?? String(error) });
  process.exitCode = 1;
} finally {
  engine?.close();
  if (built) await rm(built.dir, { recursive: true, force: true });
}
