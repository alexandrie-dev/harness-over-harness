import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ChatController } from '../../src/screens/chat/controller.mjs';
import { StubClient, RestartingStubHost, recordedPrompt, recording } from './stub-client.mjs';

const waitUntil = async condition => {
  const end = performance.now() + 5000;
  while (!condition()) {
    if (performance.now() > end) throw new Error('Condition timed out');
    await new Promise(resolve => setTimeout(resolve, 1));
  }
};
function barrier() {
  const waiters = [];
  return { frame: () => new Promise(resolve => waiters.push(resolve)), release: () => waiters.splice(0).forEach(resolve => resolve()) };
}
function transport(stub, request = (method, params) => stub.request(method, params)) {
  return { request, subscribe: async listener => stub.subscribe(listener) };
}
async function withController(callback, options = {}) {
  const stub = await StubClient.open('text-turn', options.extra ?? []);
  const frames = barrier(); const updates = [];
  const client = options.transport ? options.transport(stub) : transport(stub);
  const controller = new ChatController(() => client, state => updates.push(state), options.immediate ? async () => {} : frames.frame);
  try { await controller.connect(); await callback(controller, stub, frames, updates); }
  finally { controller.disconnect(); frames.release(); await stub.close(); }
}

test('controller connects with the typed protocol, loads history, and checks local data', async () => {
  await withController(async (controller, stub) => {
    assert.equal(controller.state.connection, 'ready'); assert.equal(controller.state.privacy, 'local');
    assert.deepEqual(controller.state.entries, []);
    const methods = stub.requests.map(r => r.method);
    for (const method of ['hoh/employee/open', 'session/working-directory/get', 'session/events', 'hoh/security/summary']) assert(methods.includes(method));
  });
});

test('controller publishes partial text before the complete assistant message', async () => {
  await withController(async (controller, stub, frames, updates) => {
    controller.setDraft(await recordedPrompt('text-turn')); await controller.send();
    await waitUntil(() => controller.state.entries.some(e => e.kind === 'assistant'));
    assert.equal(controller.state.entries.find(e => e.kind === 'assistant').text, 'SD');
    assert.equal(controller.state.busy, true);
    for (let i = 0; i < 4; i++) { frames.release(); await new Promise(resolve => setTimeout(resolve, 1)); }
    await waitUntil(() => !controller.state.busy);
    assert.equal(controller.state.entries.find(e => e.kind === 'assistant').text, 'SDK snapshot OK');
    assert.equal(controller.state.entries.filter(e => e.kind === 'user').length, 1);
    const texts = updates.map(s => s.entries.find(e => e.kind === 'assistant')?.text).filter(Boolean);
    assert(texts.includes('SD') && texts.includes('SDK') && texts.includes('SDK snapshot OK'));
  });
});

test('Stop retains the full received message, sends real cancel, and ignores later events', async () => {
  await withController(async (controller, stub, frames) => {
    controller.setDraft(await recordedPrompt('text-turn')); await controller.send();
    await waitUntil(() => controller.state.entries.some(e => e.kind === 'assistant'));
    await controller.stop();
    const stopped = controller.state.entries;
    assert.equal(controller.state.busy, false); assert.equal(controller.state.canceling, false);
    assert.equal(stopped.find(e => e.kind === 'assistant').text, 'SDK snapshot OK');
    assert.equal(stopped.find(e => e.kind === 'assistant').streaming, false);
    assert.equal(stub.requests.filter(r => r.method === 'session/cancel').length, 1);
    controller.enqueue({ method: 'session.event', params: { sessionId: 'session-1', event: {
      seq: 9999, type: 'assistant/message', time: 0, data: { message: { id: 'late', content: [{ type: 'text', text: 'Не должно появиться' }] } },
    } } });
    frames.release(); await controller.queue;
    assert.equal(controller.state.entries, stopped);
  });
});

test('late prompt receipt cannot resume playback after Stop', async () => {
  let releaseReceipt;
  const receipt = new Promise(resolve => { releaseReceipt = resolve; });
  await withController(async (controller, stub, frames) => {
    controller.setDraft(await recordedPrompt('text-turn')); const sending = controller.send();
    await waitUntil(() => controller.state.entries.some(e => e.kind === 'assistant'));
    await controller.stop(); releaseReceipt(); await sending; frames.release(); await controller.queue;
    assert.equal(controller.state.busy, false);
    assert.equal(controller.state.entries.find(e => e.kind === 'assistant').text, 'SDK snapshot OK');
  }, { transport: stub => transport(stub, async (method, params) => {
    const result = await stub.request(method, params);
    if (method === 'session/prompt') await receipt;
    return result;
  }) });
});

test('failed cancellation keeps Stop available and blocks a second prompt until acknowledged', async () => {
  let failOnce = true;
  await withController(async (controller, stub) => {
    controller.setDraft(await recordedPrompt('text-turn')); await controller.send();
    await waitUntil(() => controller.state.entries.some(e => e.kind === 'assistant'));
    await controller.stop();
    assert.equal(controller.state.busy, true); assert(controller.state.error.includes('Стоп'));
    const entries = controller.state.entries;
    controller.enqueue({ method: 'session.event', params: { sessionId: 'session-1', event: { seq: 9000, type: 'turn/start', data: {} } } });
    controller.enqueue({ method: 'session.event', params: { sessionId: 'session-1', event: { seq: 9001, type: 'assistant/message', data: { message: { id: 'late', content: [{ type: 'text', text: 'Поздний ответ отменённого хода' }] } } } } });
    await controller.queue;
    assert.equal(controller.state.entries, entries, 'late turn/start cannot reopen cancelled playback');
    const before = stub.requests.filter(r => r.method === 'session/prompt').length;
    controller.setDraft('Второе сообщение'); await controller.send();
    assert.equal(stub.requests.filter(r => r.method === 'session/prompt').length, before);
    await controller.stop(); assert.equal(controller.state.busy, false);
  }, { transport: stub => transport(stub, (method, params) => {
    if (method === 'session/cancel' && failOnce) { failOnce = false; return Promise.reject(new Error('private host detail /etc/secret')); }
    return stub.request(method, params);
  }) });
});

test('a rejected prompt is marked unsent, restores the draft, and can be retried', async () => {
  await withController(async controller => {
    controller.setDraft('Не совпадает с записанным вводом'); await controller.send();
    assert.equal(controller.state.busy, false);
    assert.equal(controller.state.draft, 'Не совпадает с записанным вводом');
    assert.equal(controller.state.entries[0].failed, true);
    assert(controller.state.error.includes('не отправлено'));
    assert(!controller.state.error.includes('recording'));
    controller.setDraft(await recordedPrompt('text-turn')); await controller.send();
    await waitUntil(() => !controller.state.busy);
    assert.equal(controller.state.entries.filter(e => e.kind === 'assistant').length, 1);
  }, { immediate: true });
});

test('repeated submission while busy sends exactly one prompt', async () => {
  await withController(async (controller, stub) => {
    controller.setDraft(await recordedPrompt('text-turn'));
    const one = controller.send(); controller.setDraft('Не отправлять'); const two = controller.send();
    await Promise.all([one, two]);
    assert.equal(stub.requests.filter(r => r.method === 'session/prompt').length, 1);
    assert.equal(controller.state.draft, 'Не отправлять');
  });
});

test('live events buffered while reading history are deduplicated against that history', async () => {
  let first = true;
  await withController(async controller => {
    await controller.queue;
    assert.equal(controller.state.entries.filter(e => e.kind === 'user').length, 1);
    assert.equal(controller.state.entries.filter(e => e.kind === 'assistant').length, 1);
    assert.equal(controller.state.busy, false);
  }, { immediate: true, transport: stub => transport(stub, async (method, params) => {
    if (method === 'session/events' && first) {
      first = false;
      await stub.request('session/prompt', { sessionId: 'session-1', contentBlocks: [{ type: 'text', text: await recordedPrompt('text-turn') }] });
      await stub.request('session/wait', { sessionId: 'session-1' });
    }
    return stub.request(method, params);
  }) });
});

test('notifications for a different conversation never change the current one', async () => {
  await withController(async controller => {
    const before = controller.state.entries;
    controller.enqueue({ method: 'session.status', params: { sessionId: 'different', status: 'running' } });
    controller.enqueue({ method: 'session.event', params: { sessionId: 'different', event: { seq: 0, type: 'tool/call', data: { name: 'bash', callId: 'other' } } } });
    await controller.queue; assert.equal(controller.state.entries, before); assert.equal(controller.state.busy, false);
  });
});

test('disconnect removes the listener and stale scheduled work cannot publish', async () => {
  await withController(async (controller, stub, frames, updates) => {
    controller.setDraft(await recordedPrompt('text-turn')); await controller.send();
    await waitUntil(() => controller.state.entries.some(e => e.kind === 'assistant'));
    controller.disconnect(); const count = updates.length; frames.release();
    await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(updates.length, count); assert.equal(stub.listeners.size, 0);
  });
});

test('connection failures expose a plain message, never private host diagnostics', async () => {
  const controller = new ChatController(() => { throw new Error('/etc/secret token SESSION'); });
  await controller.connect();
  assert.equal(controller.state.connection, 'error'); assert.equal(controller.state.privacy, 'unknown');
  assert(!/secret|token|SESSION|\/etc/.test(controller.state.error));
});

test('5000 real stub history messages are loaded across exclusive-cursor pages', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'hoh-chat-history-'));
  try {
    const rows = await recording('text-turn');
    const history = Array.from({ length: 5000 }, (_, i) => ({ type: 'user/message', data: { id: 'history-' + i, source: { kind: 'user' }, content: [{ type: 'text', text: 'Сообщение ' + i }] } }));
    const path = join(directory, 'history.jsonl');
    await writeFile(path, [rows[0], ...history, ...rows.slice(1)].map(row => JSON.stringify(row)).join('\n') + '\n');
    await withController(async (controller, stub) => {
      assert.equal(controller.state.entries.length, 5000);
      assert.equal(controller.state.entries[0].text, 'Сообщение 0');
      assert.equal(controller.state.entries[4999].text, 'Сообщение 4999');
      const requests = stub.requests.filter(r => r.method === 'session/events');
      assert.equal(requests.length, 6);
      assert(requests.slice(1).every((r, i) => r.params.afterSeq === 999 + 1000 * i));
    }, { immediate: true, extra: ['--recording', path] });
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('failed Stop stays blocked even when a late prompt receipt arrives', async () => {
  let release;
  const receipt = new Promise(resolve => { release = resolve; });
  await withController(async controller => {
    controller.setDraft(await recordedPrompt('text-turn')); const sending = controller.send();
    await waitUntil(() => controller.state.entries.some(e => e.kind === 'assistant'));
    await controller.stop(); assert.equal(controller.state.busy, true);
    release(); await sending;
    assert.equal(controller.state.busy, true, 'unconfirmed cancellation must not be overwritten by receipt');
  }, { transport: stub => transport(stub, async (method, params) => {
    if (method === 'session/cancel') throw new Error('temporary cancellation failure');
    const result = await stub.request(method, params);
    if (method === 'session/prompt') await receipt;
    return result;
  }) });
});

test('late prompt rejection after Stop does not restore an already cancelled draft', async () => {
  let reject;
  const receipt = new Promise((_, rejectReceipt) => { reject = rejectReceipt; });
  await withController(async controller => {
    controller.setDraft(await recordedPrompt('text-turn')); const sending = controller.send();
    await waitUntil(() => controller.state.entries.some(e => e.kind === 'assistant'));
    await controller.stop(); reject(new Error('cancelled request')); await sending;
    assert.equal(controller.state.busy, false); assert.equal(controller.state.draft, '');
    assert.equal(controller.state.error, ''); assert(!controller.state.entries[0].failed);
  }, { transport: stub => transport(stub, async (method, params) => {
    const result = await stub.request(method, params);
    if (method === 'session/prompt') await receipt;
    return result;
  }) });
});

test('prompt preserves intentional indentation and trailing newlines', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'hoh-chat-whitespace-'));
  try {
    const rows = await recording('text-turn');
    const padded = '    ' + await recordedPrompt('text-turn') + '\n';
    for (const row of rows) {
      if (row.type === 'agent/inbox/spliced') for (const message of row.data.inserted ?? [])
        if (message.source?.kind === 'user') message.content = [{ type: 'text', text: padded }];
      if (row.type === 'user/message' && row.data.source?.kind === 'user') row.data.content = [{ type: 'text', text: padded }];
    }
    const path = join(directory, 'whitespace.jsonl');
    await writeFile(path, rows.map(row => JSON.stringify(row)).join('\n') + '\n');
    await withController(async (controller, stub) => {
      controller.setDraft('  \n'); await controller.send();
      assert.equal(stub.requests.filter(r => r.method === 'session/prompt').length, 0);
      controller.setDraft(padded); await controller.send();
      await waitUntil(() => !controller.state.busy);
      assert.equal(stub.requests.find(r => r.method === 'session/prompt').params.contentBlocks[0].text, padded);
      assert.equal(controller.state.error, '');
      assert.equal(controller.state.entries.find(e => e.kind === 'assistant').text, 'SDK snapshot OK');
    }, { immediate: true, extra: ['--recording', path] });
  } finally { await rm(directory, { recursive: true, force: true }); }
});


test('Stop drains received backlog; reconnect keeps exactly the same messages and work', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'hoh-chat-backlog-'));
  try {
    const rows = await recording('text-turn');
    const end = rows.findIndex(row => row.type === 'turn/end');
    rows.splice(end, 0,
      { type: 'tool/call', data: { turn: 1, name: 'read_file', callId: 'queued', arguments: { path: 'note.txt' } } },
      { type: 'tool/result', data: { turn: 1, message: { toolCallId: 'queued', content: [{ type: 'text', text: 'Содержимое файла' }] } } },
      { type: 'assistant/message', data: { message: { id: 'queued-answer', content: [{ type: 'text', text: 'Второй полученный ответ' }] } } });
    const path = join(directory, 'backlog.jsonl');
    await writeFile(path, rows.map(row => JSON.stringify(row)).join('\n') + '\n');
    await withController(async (controller, stub, frames) => {
      controller.setDraft(await recordedPrompt('text-turn')); await controller.send();
      await stub.request('session/wait', { sessionId: 'session-1' });
      await waitUntil(() => controller.state.entries.some(entry => entry.streaming));
      await controller.stop();
      const content = () => controller.state.entries.map(entry => entry.kind === 'step'
        ? { kind: entry.kind, label: entry.label, detail: entry.detail, status: entry.status }
        : { kind: entry.kind, text: entry.text });
      const stopped = content();
      assert.deepEqual(stopped.map(entry => entry.text).filter(Boolean), [await recordedPrompt('text-turn'), 'SDK snapshot OK', 'Второй полученный ответ']);
      assert.equal(stopped.find(entry => entry.kind === 'step').detail, 'Содержимое файла');
      await controller.connect(); frames.release(); await controller.queue;
      assert.deepEqual(content(), stopped);
      assert.equal(controller.state.busy, false);
    }, { extra: ['--recording', path] });
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('engine-status after acknowledged prompt exits busy and recovers real stub history', async () => {
  const host = await RestartingStubHost.open();
  const controller = new ChatController(() => ({
    request: (method, params) => host.request(method, params),
    subscribe: async listener => host.subscribe(listener),
    subscribeStatus: async listener => host.subscribeStatus(listener),
  }), () => {}, async () => {});
  try {
    await controller.connect();
    assert.equal(controller.state.entries.length, 2);
    controller.setDraft(await recordedPrompt('text-turn')); await controller.send();
    await waitUntil(() => controller.state.connection === 'recovering');
    assert.equal(host.client.child.signalCode, 'SIGKILL');
    assert.equal(controller.state.busy, false); assert.equal(controller.state.canceling, false);
    assert.equal(controller.state.error, 'Связь с Помощником прервалась, восстанавливаю…');
    assert.equal(controller.state.entries.filter(entry => entry.kind === 'user').length, 2);
    controller.setDraft('Черновик остаётся'); await controller.send();
    assert.equal(host.requests.filter(request => request.method === 'session/prompt').length, 1);
    await host.restart({ unfinished: true }); await waitUntil(() => controller.state.connection === 'ready');
    assert.equal(controller.state.draft, 'Черновик остаётся'); assert.equal(controller.state.error, '');
    assert.deepEqual(controller.state.entries.filter(entry => entry.kind !== 'step').map(entry => entry.text), ['Предыдущий вопрос', 'Предыдущий ответ', await recordedPrompt('text-turn'), 'Сохранённая часть ответа']);
    assert.equal(controller.state.busy, false);
    assert.equal(controller.state.entries.find(entry => entry.kind === 'step').status, 'stopped');
    controller.setDraft(await recordedPrompt('text-turn')); await controller.send();
    await waitUntil(() => !controller.state.busy);
    assert.equal(controller.state.entries.filter(entry => entry.kind === 'assistant').length, 3);
    assert.equal(controller.state.entries.at(-1).text, 'SDK snapshot OK');
    assert.equal(host.requests.filter(request => request.method === 'session/events').length, 2);
  } finally {
    controller.disconnect(); assert.equal(host.statusListeners.size, 0); await host.close();
  }
});


test('rapid unmount/remount releases a late lifecycle subscription exactly once', async () => {
  const stub = await StubClient.open(); const pending = []; const listeners = new Set();
  const client = { ...transport(stub), subscribeStatus: listener => new Promise(resolve => {
    listeners.add(listener); pending.push(() => resolve(() => listeners.delete(listener)));
  }) };
  const controller = new ChatController(() => client);
  try {
    const first = controller.connect(); controller.disconnect(); const second = controller.connect();
    pending.splice(0).forEach(resolve => resolve()); await Promise.all([first, second]);
    assert.equal(listeners.size, 1); assert.equal(controller.state.connection, 'ready');
    controller.disconnect(); assert.equal(listeners.size, 0);
  } finally { controller.disconnect(); await stub.close(); }
});

test('ready retries a history load that failed while the engine was restarting', async () => {
  const stub = await StubClient.open(); let listener; let unavailable = true;
  const client = { ...transport(stub, (method, params) => {
    if (method === 'hoh/employee/open' && unavailable) {
      unavailable = false; return Promise.reject(new Error('engine still starting /etc/secret'));
    }
    return stub.request(method, params);
  }), subscribeStatus: async callback => { listener = callback; return () => { listener = undefined; }; } };
  const controller = new ChatController(() => client);
  try {
    await controller.connect(); assert.equal(controller.state.connection, 'error');
    listener({ state: 'ready' }); await waitUntil(() => controller.state.connection === 'ready');
    assert.equal(controller.state.error, ''); assert.equal(controller.state.busy, false);
  } finally { controller.disconnect(); await stub.close(); }
});
