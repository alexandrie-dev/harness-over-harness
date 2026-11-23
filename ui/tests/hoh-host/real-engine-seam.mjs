// Production HohHost driven by the Rust desktop seam over JSON lines. Rust owns the real engine,
// main-window confirmation and approval Broker; this file owns exactly the T3 host/client side.
import assert from 'node:assert/strict';
import { createInterface } from 'node:readline';
import { rm } from 'node:fs/promises';
import { buildHost } from './build.mjs';
import { WireClient } from './wire-client.mjs';

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
      frame.error ? pending.reject(new Error(frame.error)) : pending.resolve(frame.result);
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
  request(method, params) { return this.call('request', { method, params }); }
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
  const host = new mod.host.HohHost({ engine, throttleMs: 0, loadRetryMs: 10, loadRetries: 6000, storage: null });
  const server = new mod.wire.WireServer(host.handlers());
  const client = new WireClient(mod.socket.HohWebSocket, server);
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
  assert.equal(launched.projection.thread.runtimeMode, 'auto');

  const pending = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.some(request => request.status === 'pending'), 0, 60_000);
  const request = pending.projection.runtimeRequests.find(candidate => candidate.status === 'pending');
  const card = pending.projection.turnItems.find(item => item.type === 'approval_request' && item.requestId === request.id);
  assert(card, 'the native pending request must become the production T3 card');
  assert.equal(typeof card.prompt, 'string');
  assert(card.prompt.length > 0, 'the card must show engine humanText');
  assert.equal(card.requestId, request.id);

  // The native Broker deliberately refuses an instant allow before the person could read the card.
  await new Promise(resolve => setTimeout(resolve, 1_100));
  await client.call('orchestration.dispatchCommand', {
    type: 'runtime-request.respond', commandId: 'approve-real-seam', threadId,
    requestId: request.id, decision: 'accept',
  });
  const resolved = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.some(candidate => candidate.id === request.id && candidate.status === 'resolved'), 0, 60_000);
  assert.equal(resolved.projection.runtimeRequests.find(candidate => candidate.id === request.id).decision, 'accept');
  const access = await engine.request('hoh/executor/access/get', { executor: 'claude', sessionId: threadId });
  assert.equal(access.mode, 'auto');

  host.stop();
  client.close();
  write({ type: 'result', ok: true, approvalId: request.id, humanText: card.prompt, mode: access.mode });
} catch (error) {
  write({ type: 'result', ok: false, error: error?.stack ?? String(error) });
  process.exitCode = 1;
} finally {
  engine?.close();
  if (built) await rm(built.dir, { recursive: true, force: true });
}
