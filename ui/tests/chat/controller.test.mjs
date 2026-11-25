import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ChatController } from '../../src/screens/chat/controller.mjs';
import { StubClient, RestartingStubHost, recordedPrompt, recording } from './stub-client.mjs';
import { FakeEngine } from './fake-engine.mjs';

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
      const content = () => controller.state.entries.filter(entry => entry.kind !== 'note').map(entry => entry.kind === 'step'
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

test('a window-profile engine without employees keeps one conversation and judges privacy by model address', async () => {
  const stub = await StubClient.open(); const asked = [];
  const missing = () => Promise.reject(new Error('method-unavailable'));
  const address = { value: 'http://127.0.0.1:8080/v1' };
  const client = transport(stub, (method, params) => {
    asked.push({ method, params });
    if (method === 'hoh/employee/open' || method === 'hoh/security/summary') return missing();
    if (method === 'hoh/settings/get') return Promise.resolve({ model: { baseUrl: address.value, model: 'm' } });
    return stub.request(method, params);
  });
  const controller = new ChatController(() => client);
  try {
    await controller.connect();
    assert.equal(controller.sessionId, 'window-main');
    assert(asked.some(r => r.method === 'session/working-directory/get' && r.params.sessionId === 'window-main'));
    assert.equal(controller.state.privacy, 'local');
    address.value = 'https://api.example.com/v1';
    await controller.connect();
    assert.equal(controller.state.privacy, 'remote');
    address.value = 'http://localhost.evil.example/v1';
    await controller.connect();
    assert.equal(controller.state.privacy, 'remote');
  } finally { controller.disconnect(); await stub.close(); }
});

test('after a restart the saved conversation is resumed before its folder is asked for, and shown', async () => {
  const stub = await StubClient.open('text-turn'); const asked = [];
  await stub.request('session/resume', { sessionId: 'window-main' });
  await stub.request('session/prompt', { sessionId: 'window-main', contentBlocks: [{ type: 'text', text: await recordedPrompt('text-turn') }] });
  await stub.request('session/wait', { sessionId: 'window-main' });
  // the real engine: a session on disk that is not open yet cannot be reached except by resume
  let open = false;
  const client = transport(stub, (method, params) => {
    asked.push(method);
    if (method === 'hoh/employee/open' || method === 'hoh/security/summary') return Promise.reject(new Error('method-unavailable'));
    if (method === 'hoh/settings/get') return Promise.resolve({ model: { baseUrl: 'http://127.0.0.1:8080/v1', model: 'm' } });
    if (method === 'session/resume') { open = true; return Promise.resolve({}); }
    if (method === 'session/working-directory/get' && !open) return Promise.reject(new Error('session "window-main" already exists'));
    return stub.request(method, params);
  });
  const controller = new ChatController(() => client, undefined, async () => {});
  try {
    await controller.connect();
    assert.equal(controller.state.connection, 'ready');
    assert(controller.state.entries.some(e => e.kind === 'user'), 'прежняя история на экране');
    assert(controller.state.entries.some(e => e.kind === 'assistant'));
    assert(asked.indexOf('session/resume') < asked.indexOf('session/working-directory/get'));
  } finally { controller.disconnect(); await stub.close(); }
});

test('a failing resume (new session, already open) does not stop the connection', async () => {
  const stub = await StubClient.open();
  const client = transport(stub, (method, params) => method === 'session/resume' ? Promise.reject(new Error('session "window-main" not found')) : stub.request(method, params));
  const controller = new ChatController(() => client);
  try { await controller.connect(); assert.equal(controller.state.connection, 'ready'); }
  finally { controller.disconnect(); await stub.close(); }
});

test('only the missing-method marker triggers the fallback', async () => {
  const stub = await StubClient.open();
  const client = transport(stub, (method, params) => method === 'hoh/employee/open' ? Promise.reject(new Error('Помощник отклонил запрос')) : stub.request(method, params));
  const controller = new ChatController(() => client);
  try { await controller.connect(); assert.equal(controller.state.connection, 'error'); }
  finally { controller.disconnect(); await stub.close(); }
});

test('the answer grows part by part while the model writes, keeps what came before Stop, and the final message replaces it', async () => {
  const stub = await StubClient.open(); let emit;
  const client = { ...transport(stub), subscribe: async listener => { emit = listener; return () => { emit = undefined; }; } };
  const controller = new ChatController(() => client, undefined, async () => {});
  try {
    await controller.connect();
    const sessionId = controller.sessionId;
    const push = async (method, params) => { emit({ method, params: { sessionId, ...params } }); await controller.queue; };
    const answer = () => controller.state.entries.filter(e => e.kind === 'assistant');
    const chunk = (index, text) => push('session.assistant-stream', { frame: { type: 'chunk', attemptId: 'a1', revision: 2 + index, index, time: 1, chunk: { type: 'text-delta', text } } });
    await push('session.status', { status: 'running' });
    await push('session.assistant-stream', { frame: { type: 'start', attemptId: 'a1', revision: 1, turn: 1, step: 1 } });
    assert.equal(answer().length, 0, 'пустого ответа до первой части нет');
    await chunk(0, 'Я умею ');
    assert.deepEqual(answer().map(e => [e.text, e.streaming]), [['Я умею ', true]]);
    await chunk(1, 'читать файлы');
    assert.equal(answer()[0].text, 'Я умею читать файлы', 'текст растёт');
    await chunk(1, 'ПОВТОР');
    assert.equal(answer()[0].text, 'Я умею читать файлы', 'повтор кадра не удваивает текст');
    assert(controller.state.busy);
    await controller.stop();
    assert.deepEqual(answer().map(e => [e.text, e.streaming]), [['Я умею читать файлы', false]], 'после «Стоп» остаётся полученная часть');
    await chunk(2, ' ПОЗДНО');
    assert.equal(answer()[0].text, 'Я умею читать файлы', 'поздние части остановленного ответа не возвращаются');
  } finally { controller.disconnect(); await stub.close(); }
});

async function liveSetup() {
  const stub = await StubClient.open(); let emit;
  const client = { ...transport(stub), subscribe: async listener => { emit = listener; return () => { emit = undefined; }; } };
  const controller = new ChatController(() => client, undefined, async () => {});
  await controller.connect();
  const sessionId = controller.sessionId;
  const push = async (params, session = sessionId, method = 'session.assistant-stream') => { emit({ method, params: { sessionId: session, ...params } }); await controller.queue; };
  const start = (attemptId, revision, step = 1) => push({ frame: { type: 'start', attemptId, revision, turn: 1, step } });
  const chunk = (attemptId, revision, index, text) => push({ frame: { type: 'chunk', attemptId, revision, index, time: 1, chunk: { type: 'text-delta', text } } });
  const shown = () => controller.state.entries.filter(e => e.kind === 'assistant').map(e => e.text);
  return { stub, controller, push, start, chunk, shown, sessionId, close: async () => { controller.disconnect(); await stub.close(); } };
}

test('live frames: another session is ignored, revision is per attempt, a replaced attempt never comes back', async () => {
  const t = await liveSetup();
  try {
    await t.push({ frame: { type: 'start', attemptId: 'x', revision: 1, turn: 1, step: 1 } }, 'another-session');
    await t.push({ frame: { type: 'chunk', attemptId: 'x', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'ЧУЖАЯ' } } }, 'another-session');
    assert.deepEqual(t.shown(), [], 'кадры чужой сессии не рисуются');

    await t.start('a1', 1); await t.chunk('a1', 1, 0, 'первая ');
    assert.deepEqual(t.shown(), ['первая '], 'первая часть с той же ревизией, что и start, не теряется');
    await t.chunk('a1', 1, 1, 'и вторая ');
    assert.deepEqual(t.shown(), ['первая и вторая '], 'ревизия может не расти на каждом кадре');
    await t.chunk('a1', 1, 1, 'ПОВТОР ИНДЕКСА');
    await t.chunk('a1', 0, 2, 'ОТКАТ РЕВИЗИИ');
    assert.deepEqual(t.shown(), ['первая и вторая '], 'повтор индекса и откат ревизии внутри попытки игнорируются');
    await t.chunk('a1', 5, 2, 'третья');
    assert.deepEqual(t.shown(), ['первая и вторая третья']);

    // a retry of the step restarts the revision at 1: the new attempt is shown, the text starts over
    await t.start('a2', 1); await t.chunk('a2', 1, 0, 'вторая попытка');
    assert.deepEqual(t.shown(), ['вторая попытка'], 'новая попытка с revision 1 после revision 5 видна');
    // late frames of the replaced attempt are dropped whatever their revision
    await t.start('a1', 9);
    await t.chunk('a1', 9, 3, ' ПОЗДНО');
    await t.chunk('a1', 1, 4, ' ПОЗДНО 2');
    assert.deepEqual(t.shown(), ['вторая попытка'], 'запоздалая попытка a1 не возвращается');
    await t.chunk('a2', 1, 1, ' продолжается');
    assert.deepEqual(t.shown(), ['вторая попытка продолжается'], 'текущая попытка продолжает расти');
  } finally { await t.close(); }
});

test('frames after the end of an attempt and after the final message do not change the answer', async () => {
  const t = await liveSetup();
  try {
    await t.start('a1', 1); await t.chunk('a1', 2, 0, 'Готово');
    await t.push({ frame: { type: 'end', attemptId: 'a1', revision: 3, index: 1, outcome: 'committed' } });
    await t.chunk('a1', 4, 1, ' ПОСЛЕ КОНЦА');
    assert.deepEqual(t.shown(), ['Готово']);
    await t.push({ event: { type: 'assistant/message', seq: 901, time: 1, data: { turn: 1, step: 1, message: { id: 'm-9', role: 'assistant', content: [{ type: 'text', text: 'Готово.' }] } } } }, t.sessionId, 'session.event');
    assert.deepEqual(t.shown(), ['Готово.']);
    await t.start('a1', 9); await t.chunk('a1', 10, 0, 'ВОСКРЕС');
    assert.deepEqual(t.shown(), ['Готово.'], 'после итогового сообщения живой текст не возвращается');
  } finally { await t.close(); }
});

test('the durable assistant message takes the place of the live text without a second copy', async () => {
  const stub = await StubClient.open(); let emit;
  const client = { ...transport(stub), subscribe: async listener => { emit = listener; return () => { emit = undefined; }; } };
  const controller = new ChatController(() => client, undefined, async () => {});
  try {
    await controller.connect();
    const sessionId = controller.sessionId;
    const push = async (method, params) => { emit({ method, params: { sessionId, ...params } }); await controller.queue; };
    await push('session.status', { status: 'running' });
    await push('session.assistant-stream', { frame: { type: 'start', attemptId: 'a1', revision: 1, turn: 1, step: 1 } });
    await push('session.assistant-stream', { frame: { type: 'chunk', attemptId: 'a1', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Готово' } } });
    await push('session.assistant-stream', { frame: { type: 'end', attemptId: 'a1', revision: 3, index: 1, outcome: 'committed' } });
    await push('session.event', { event: { type: 'assistant/message', seq: 900, time: 1, data: { turn: 1, step: 1, message: { id: 'm-1', role: 'assistant', content: [{ type: 'text', text: 'Готово.' }] } } } });
    const answers = controller.state.entries.filter(e => e.kind === 'assistant');
    assert.deepEqual(answers.map(e => [e.id, e.text, e.streaming]), [['assistant-m-1', 'Готово.', false]]);
  } finally { controller.disconnect(); await stub.close(); }
});

async function withGapController(history, callback, { failCancelOnce = false } = {}) {
  const stub = await StubClient.open(); let emit; let historyReads = 0; let waitReads = 0; let cancelAttempts = 0;
  const client = transport(stub, (method, params) => {
    if (method === 'session/cancel' && failCancelOnce && cancelAttempts++ === 0) return Promise.reject(new Error('temporary cancellation failure'));
    if (method === 'session/wait') waitReads++;
    if (method === 'session/events') {
      historyReads++;
      if (historyReads > 1) return Promise.resolve({ events: history, nextSeq: history.at(-1)?.seq ?? -1 });
    }
    return stub.request(method, params);
  });
  client.subscribe = async listener => { emit = listener; return () => { emit = undefined; }; };
  const controller = new ChatController(() => client, undefined, async () => {});
  try {
    await controller.connect(); const sessionId = controller.sessionId;
    const push = async (method, params) => { emit({ method, params: { sessionId, ...params } }); await controller.queue; };
    await callback(controller, push, () => historyReads, () => waitReads);
  } finally { controller.disconnect(); await stub.close(); }
}

async function sendGappedTurn(push, end = true) {
  await push('session.status', { status: 'running' });
  await push('session.assistant-stream', { frame: { type: 'start', attemptId: 'gap-1', revision: 1, turn: 7, step: 2 } });
  await push('session.assistant-stream', { frame: { type: 'chunk', attemptId: 'gap-1', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Префикс' } } });
  await push('session.assistant-stream', { frame: { type: 'gap', attemptId: 'gap-1', revision: 3, index: 1 } });
  await push('session.assistant-stream', { frame: { type: 'chunk', attemptId: 'gap-1', revision: 4, index: 2, time: 1, chunk: { type: 'text-delta', text: ' ЛИШНЕЕ' } } });
  if (end) await push('session.assistant-stream', { frame: { type: 'end', attemptId: 'gap-1', revision: 5, index: 3, outcome: 'committed' } });
}

test('turn end reconciles a gapped live prefix from durable session history', async () => {
  const final = { type: 'assistant/message', seq: 700, time: 1, data: { turn: 7, step: 2, message: { id: 'gap-final', content: [{ type: 'text', text: 'Полный ответ' }] } } };
  await withGapController([final], async (controller, push, historyReads) => {
    await sendGappedTurn(push);
    assert.deepEqual(controller.state.entries.map(entry => [entry.text, entry.streaming]), [['Префикс', true]]);
    await push('session.event', { event: { type: 'turn/end', seq: 701, time: 1, data: { reason: { kind: 'completed' } } } });
    assert.deepEqual(controller.state.entries, [{ id: 'assistant-gap-final', kind: 'assistant', text: 'Полный ответ', streaming: false }]);
    assert.equal(controller.state.busy, false); assert.equal(historyReads(), 2);
  });
});

test('turn end marks an unrecoverable gapped prefix as incomplete instead of final', async () => {
  await withGapController([], async (controller, push, historyReads) => {
    await sendGappedTurn(push);
    await push('session.event', { event: { type: 'turn/end', seq: 702, time: 1, data: { reason: { kind: 'error' } } } });
    assert.deepEqual(controller.state.entries.map(entry => [entry.text, entry.streaming, entry.incomplete]), [['Префикс\n\n*Ответ получен не полностью.*', false, true]]);
    assert.equal(controller.state.busy, false); assert.equal(historyReads(), 2);
  });
});

test('Stop waits for cancellation and replaces a gapped prefix from durable history', async () => {
  const final = { type: 'assistant/message', seq: 710, time: 1, data: { turn: 7, step: 2, message: { id: 'cancel-final', content: [{ type: 'text', text: 'Сохранённый ответ' }] } } };
  await withGapController([final], async (controller, push, historyReads) => {
    await sendGappedTurn(push, false);
    await controller.stop();
    assert.deepEqual(controller.state.entries.filter(entry => entry.kind !== 'note'), [{ id: 'assistant-cancel-final', kind: 'assistant', text: 'Сохранённый ответ', streaming: false }]);
    assert.equal(controller.state.entries.filter(entry => entry.kind === 'note').length, 1, '«Вы остановили ответ» — один раз');
    assert.equal(controller.state.busy, false); assert.equal(controller.state.canceling, false); assert.equal(historyReads(), 2);
  });
});

test('a successful Stop retry still recovers a gapped prefix after cancellation failed once', async () => {
  const final = { type: 'assistant/message', seq: 711, time: 1, data: { turn: 7, step: 2, message: { id: 'retry-final', content: [{ type: 'text', text: 'Итог после повтора' }] } } };
  await withGapController([final], async (controller, push, historyReads) => {
    await sendGappedTurn(push, false);
    await controller.stop();
    assert.equal(controller.state.busy, true); assert.equal(controller.state.canceling, false);
    assert.equal(controller.state.entries[0].streaming, true); assert.equal(controller.projection.hasGap(), true);
    await controller.stop();
    assert.deepEqual(controller.state.entries.filter(entry => entry.kind !== 'note'), [{ id: 'assistant-retry-final', kind: 'assistant', text: 'Итог после повтора', streaming: false }]);
    assert.equal(controller.state.entries.filter(entry => entry.kind === 'note').length, 1, 'повторный «Стоп» не добавляет вторую пометку');
    assert.equal(controller.state.busy, false); assert.equal(historyReads(), 2);
  }, { failCancelOnce: true });
});

test('a late durable message replaces an incomplete gap without affecting a later ordinary Stop', async () => {
  await withGapController([], async (controller, push, historyReads, waitReads) => {
    await sendGappedTurn(push);
    await push('session.event', { event: { type: 'turn/end', seq: 720, time: 1, data: { reason: { kind: 'completed' } } } });
    assert.equal(controller.projection.hasGap(), false);
    assert.equal(historyReads(), 2);
    await push('session.event', { event: { type: 'assistant/message', seq: 721, time: 1, data: { turn: 7, step: 2, message: { id: 'late-final', content: [{ type: 'text', text: 'Поздний полный ответ' }] } } } });
    assert.deepEqual(controller.state.entries.filter(entry => entry.kind === 'assistant'), [{ id: 'assistant-late-final', kind: 'assistant', text: 'Поздний полный ответ', streaming: false }]);

    await push('session.status', { status: 'running' });
    await push('session.assistant-stream', { frame: { type: 'start', attemptId: 'ordinary', revision: 1, turn: 8, step: 1 } });
    await push('session.assistant-stream', { frame: { type: 'chunk', attemptId: 'ordinary', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Обычный ответ' } } });
    await controller.stop();
    assert.equal(historyReads(), 2); assert.equal(waitReads(), 0);
  });
});

test('a late empty durable message removes an incomplete gap without affecting a later ordinary Stop', async () => {
  await withGapController([], async (controller, push, historyReads, waitReads) => {
    await sendGappedTurn(push);
    await push('session.event', { event: { type: 'turn/end', seq: 722, time: 1, data: { reason: { kind: 'completed' } } } });
    assert.equal(controller.projection.hasGap(), false); assert.equal(historyReads(), 2);
    await push('session.event', { event: { type: 'assistant/message', seq: 723, time: 1, data: { turn: 7, step: 2, message: { id: 'late-empty', content: [] } } } });
    assert.deepEqual(controller.state.entries, []);

    await push('session.status', { status: 'running' });
    await push('session.assistant-stream', { frame: { type: 'start', attemptId: 'ordinary-empty', revision: 1, turn: 8, step: 1 } });
    await push('session.assistant-stream', { frame: { type: 'chunk', attemptId: 'ordinary-empty', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Обычный ответ' } } });
    await controller.stop();
    assert.equal(historyReads(), 2); assert.equal(waitReads(), 0);
  });
});

test('history recovery removes a gapped prefix when the canonical message is empty', async () => {
  const empty = { type: 'assistant/message', seq: 730, time: 1, data: { turn: 7, step: 2, message: { id: 'empty-recovered', content: [] } } };
  await withGapController([empty], async (controller, push, historyReads) => {
    await sendGappedTurn(push);
    await push('session.event', { event: { type: 'turn/end', seq: 731, time: 1, data: { reason: { kind: 'completed' } } } });
    assert.deepEqual(controller.state.entries, []);
    assert.equal(controller.state.busy, false); assert.equal(historyReads(), 2);
  });
});

test('questions from the assistant are shown, answered through the host, and withdrawn by the engine', async () => {
  const stub = await StubClient.open(); let ask; const answered = [];
  const client = { ...transport(stub),
    subscribeQuestions: async listener => { ask = listener; return () => { ask = undefined; }; },
    answerQuestion: async (requestId, answer) => { if (answer.answers?.[0]?.selected[0] === 'refused') throw new Error('x'); answered.push({ requestId, answer }); } };
  let emit; client.subscribe = async listener => { emit = listener; return () => { emit = undefined; }; };
  const controller = new ChatController(() => client);
  try {
    await controller.connect();
    const question = { requestId: 'r1', sessionId: 'window-main', questions: [{ id: 'q1', question: 'Какой формат?', options: [{ label: 'PDF' }] }] };
    ask(question); ask(question);
    assert.equal(controller.state.questions.length, 1);
    await controller.answerQuestion('r1', { answers: [{ id: 'q1', selected: ['PDF'] }] });
    assert.deepEqual(answered, [{ requestId: 'r1', answer: { answers: [{ id: 'q1', selected: ['PDF'] }] } }]);
    assert.equal(controller.state.questions.length, 0);
    ask({ ...question, requestId: 'r2' }); ask({ ...question, requestId: 'r3' });
    emit({ method: 'user-question.cancelled', params: { requestId: 'r2', sessionId: 'window-main' } });
    assert.deepEqual(controller.state.questions.map(q => q.requestId), ['r3']);
    await controller.answerQuestion('r3', { answers: [{ id: 'q1', selected: ['refused'] }] });
    assert.equal(controller.state.questions.length, 1); assert.notEqual(controller.state.error, '');
    await controller.dismissQuestion('r3');
    assert.deepEqual(answered.at(-1), { requestId: 'r3', answer: { cancelled: true } });
    assert.equal(controller.state.questions.length, 0);
    ask({ ...question, requestId: 'r4' });
    controller.engineStatus({ state: 'restarting' });
    assert.equal(controller.state.questions.length, 0);
  } finally { controller.disconnect(); await stub.close(); }
});

// ——— список чатов: новый чат, переключение, история не смешивается ———
async function withChats(callback, options = {}) {
  const engine = new FakeEngine(options);
  Object.assign(engine, options.engine ?? {});
  engine.seed('window-main', { user: 'Первый вопрос', assistant: 'Первый ответ' });
  engine.seed('chat-old', { user: 'Старый вопрос', assistant: 'Старый ответ' });
  const client = { request: (method, params) => engine.request(method, params), subscribe: async listener => engine.subscribe(listener) };
  const controller = new ChatController(() => client, () => {}, async () => {});
  try { await controller.connect(); await waitUntil(() => controller.state.sessions.length >= (options.unavailable ? 1 : 2)); await callback(controller, engine); }
  finally { controller.disconnect(); await engine.close(); }
}
const shown = controller => controller.state.entries.map(e => e.text);

test('the sidebar lists the chats of the engine with their titles; the open one is marked', async () => {
  await withChats(async controller => {
    assert.equal(controller.state.currentId, 'window-main');
    assert.deepEqual(controller.state.sessions.map(s => s.id).sort(), ['chat-old', 'window-main']);
    assert.equal(controller.state.sessions.find(s => s.id === 'chat-old').title, 'Старый вопрос');
    assert.deepEqual(shown(controller), ['Первый вопрос', 'Первый ответ']);
  });
});

test('switching chats shows only that chat and does not mix histories', async () => {
  await withChats(async (controller, engine) => {
    const listCalls = () => engine.requests.filter(request => request.method === 'session/list').length;
    const beforeSwitch = listCalls();
    await controller.openSession('chat-old');
    assert.equal(controller.state.connection, 'ready'); assert.equal(controller.state.currentId, 'chat-old');
    assert.deepEqual(shown(controller), ['Старый вопрос', 'Старый ответ']);
    assert.equal(listCalls(), beforeSwitch, 'switching between listed chats does not reload all session logs');
    controller.setDraft('Третий вопрос'); await controller.send();
    await waitUntil(() => !controller.state.busy && controller.state.entries.some(e => e.text === 'Ответ на: Третий вопрос'));
    assert.deepEqual(shown(controller), ['Старый вопрос', 'Старый ответ', 'Третий вопрос', 'Ответ на: Третий вопрос']);
    await controller.openSession('window-main');
    assert.deepEqual(shown(controller), ['Первый вопрос', 'Первый ответ']);
    await controller.openSession('chat-old');
    assert.deepEqual(shown(controller), ['Старый вопрос', 'Старый ответ', 'Третий вопрос', 'Ответ на: Третий вопрос']);
    assert(engine.requests.some(r => r.method === 'session/resume' && r.params.sessionId === 'chat-old'));
  });
});

test('events of a chat that is not open never reach the open chat, and are there when it is opened', async () => {
  await withChats(async (controller, engine) => {
    controller.setDraft('Вопрос в первом'); await controller.send();
    await controller.openSession('chat-old');                 // leave while the turn is still running
    await waitUntil(() => engine.sessions.get('window-main').events.some(e => e.type === 'turn/end'));
    await controller.queue;
    assert.deepEqual(shown(controller), ['Старый вопрос', 'Старый ответ']);
    await controller.openSession('window-main');
    assert.deepEqual(shown(controller), ['Первый вопрос', 'Первый ответ', 'Вопрос в первом', 'Ответ на: Вопрос в первом']);
  });
});

test('a new chat starts empty, is listed, takes its title from the first message, and is not duplicated while empty', async () => {
  await withChats(async (controller, engine) => {
    await controller.newChat();
    const fresh = controller.state.currentId;
    assert.match(fresh, /^chat-/); assert.notEqual(fresh, 'window-main');
    assert.deepEqual(shown(controller), []); assert.equal(controller.state.connection, 'ready');
    await waitUntil(() => controller.state.sessions.some(s => s.id === fresh));
    assert.equal(controller.state.sessions.length, 3);
    await controller.newChat();                              // still empty: reused
    assert.equal(controller.state.currentId, fresh);
    assert.equal(engine.requests.filter(r => r.method === 'session/working-directory/get' && r.params.sessionId.startsWith('chat-') && r.params.sessionId !== 'chat-old').length, 1);
    controller.setDraft('Новая тема'); await controller.send();
    await waitUntil(() => controller.state.sessions.find(s => s.id === fresh)?.title === 'Новая тема');
    await waitUntil(() => !controller.state.busy);
    await controller.newChat();                              // written to: a second new chat
    assert.notEqual(controller.state.currentId, fresh); assert.deepEqual(shown(controller), []);
    await controller.openSession(fresh);
    assert.deepEqual(shown(controller), ['Новая тема', 'Ответ на: Новая тема']);
    await controller.openSession('window-main');
    assert(!shown(controller).includes('Новая тема'));
  });
});

test('a reconnect returns to the chat the person chose, not to the default one', async () => {
  await withChats(async controller => {
    await controller.openSession('chat-old');
    await controller.connect(true);
    assert.equal(controller.sessionId, 'chat-old'); assert.deepEqual(shown(controller), ['Старый вопрос', 'Старый ответ']);
  });
});

test('an engine without session/list keeps the open chat usable', async () => {
  await withChats(async controller => {
    assert.deepEqual(controller.state.sessions.map(s => s.id), ['window-main']);
    controller.setDraft('Привет'); await controller.send();
    await waitUntil(() => controller.state.entries.some(e => e.text === 'Ответ на: Привет'));
    assert.equal(controller.state.error, '');
  }, { unavailable: ['session/list'] });
});

test('the composer line gets model and permission mode from the engine, and a chosen folder is sent to it', async () => {
  await withChats(async (controller, engine) => {
    assert.equal(controller.state.model, 'Модель на этом компьютере'); assert.equal(controller.state.permissions, 'Спрашивать разрешение');
    await controller.setFolder('/home/me/Проекты/сайт');
    assert.equal(controller.state.folderLabel, 'сайт');
    assert.deepEqual(engine.requests.find(r => r.method === 'session/working-directory/set')?.params, { sessionId: 'window-main', path: '/home/me/Проекты/сайт' });
  });
});

test('when the engine cannot be reached the person is told why and what to do', async () => {
  for (const [failure, expected] of [[new Error('Помощник ещё подключается'), /Помощник ещё запускается/], [new Error('boom'), /Помощник не отвечает/]]) {
    const controller = new ChatController(() => ({ request: async () => { throw failure; }, subscribe: async () => () => {} }), () => {}, async () => {});
    await controller.connect();
    assert.equal(controller.state.connection, 'error');
    assert.match(controller.state.error, expected); assert.match(controller.state.error, /нажмите «Подключиться ещё раз»/);
    controller.disconnect();
  }
});

test('F-3: a new chat while another empty chat exists goes to that one instead of creating one more', async () => {
  await withChats(async controller => {
    await controller.newChat();                                   // the first empty chat
    const empty = controller.state.currentId;
    await waitUntil(() => controller.state.sessions.some(s => s.id === empty));
    await controller.openSession('chat-old');                      // back to a chat with messages
    const before = controller.state.sessions.length;
    await controller.newChat();                                   // must land in the existing empty one
    assert.equal(controller.state.currentId, empty);
    await controller.openSession('chat-old'); await controller.newChat();
    assert.equal(controller.state.sessions.length, before, 'no further empty chats');
    assert.equal(controller.state.sessions.filter(s => s.title === '').length, 1);
  });
});

test('F-1: when the host reports that the engine does not start, the window ends in an error with the reason, and recovers when it is ready', async () => {
  await withChats(async controller => {
    controller.engineStatus({ state: 'restarting' });
    assert.equal(controller.state.connection, 'recovering');
    controller.engineStatus({ state: 'failed', message: 'Помощник не запускается: файл Помощника не найден. Закройте HoH и откройте снова.' });
    assert.equal(controller.state.connection, 'error');
    assert.match(controller.state.error, /файл Помощника не найден/);
    controller.engineStatus({ state: 'ready' });
    await waitUntil(() => controller.state.connection === 'ready');
    assert.equal(controller.state.error, '');
  });
});

const accessMethods = engine => engine.requests.map(r => r.method).filter(m => m.startsWith('hoh/executor/access/'));
test('Claude access defaults to confirm, and a change is shown only after the mode is read back from the engine', async () => {
  await withChats(async (controller, engine) => {
    assert.equal(controller.state.access, 'confirm'); assert.deepEqual(accessMethods(engine), ['hoh/executor/access/get']);
    await controller.setAccess('full');
    assert.deepEqual(accessMethods(engine), ['hoh/executor/access/get', 'hoh/executor/access/set', 'hoh/executor/access/get']);
    assert.deepEqual(engine.requests.findLast(r => r.method === 'hoh/executor/access/set').params, { executor: 'claude', mode: 'full' });
    assert.equal(controller.state.access, 'full'); assert.equal(controller.state.notice, '');
    await controller.connect(true);                          // the engine, not the window, remembers the choice
    assert.equal(controller.state.access, 'full');
  }, { engine: { access: 'confirm' } });
});
test('the project-settings line follows effective.flags of the engine, not a constant of the window', async () => {
  for (const [sources, off] of [['user', true], ['user,project,local', false], ['', true], [undefined, false]]) {
    await withChats(async controller => {
      assert.equal(controller.state.executor, 'claude');
      assert.equal(controller.state.projectSettingsOff, off, `--setting-sources ${JSON.stringify(sources)}`);
      assert.equal(controller.state.personalSettings, sources !== undefined && sources.split(',').includes('user'), `personal: ${JSON.stringify(sources)}`);
    }, { engine: { access: 'confirm', settingSources: sources } });
  }
});
const sendAndWait = async (controller, text) => {
  controller.setDraft(text); await controller.send();
  await waitUntil(() => !controller.state.busy && controller.state.entries.some(e => e.text === `Ответ на: ${text}`));
};
const accessReads = engine => engine.requests.filter(r => r.method === 'hoh/executor/access/get');
test('the mode of the open chat is the mode of its next turn; a change waits for the next message and can be taken back', async () => {
  await withChats(async (controller, engine) => {
    await sendAndWait(controller, 'первое');
    assert.deepEqual([controller.state.access, controller.state.accessPending], ['confirm', false]);
    assert.equal(accessReads(engine).at(-1).params.sessionId, 'window-main', 'the engine is asked about the open chat');
    await controller.setAccess('full');
    assert.deepEqual([controller.state.access, controller.state.accessPending], ['full', true], 'chosen mode shown, change waits for the next message');
    await controller.setAccess('confirm');
    assert.deepEqual([controller.state.access, controller.state.accessPending], ['confirm', false], 'taken back before any message');
    await controller.setAccess('full');
    assert.equal(controller.state.accessPending, true);
    const before = accessReads(engine).length;
    controller.setDraft('второе'); const sending = controller.send();
    assert.equal(controller.state.accessPending, true, 'sending does not erase the line');
    await sending; await waitUntil(() => controller.state.entries.some(e => e.text === 'Ответ на: второе') && !controller.state.busy);
    assert.deepEqual([controller.state.access, controller.state.accessPending], ['full', false], 'the next turn runs in the chosen mode');
    assert(accessReads(engine).length >= before + 2, 'read again at the start and at the end of the turn');
    await controller.setAccess('confirm');
    assert.deepEqual([controller.state.access, controller.state.accessPending], ['confirm', true], 'the other way round');
  }, { engine: { access: 'confirm' } });
});
test('two chats keep their own mode: each shows the mode of its own process', async () => {
  await withChats(async (controller, engine) => {
    await sendAndWait(controller, 'в первом');
    await controller.setAccess('full');
    await controller.openSession('chat-old'); await waitUntil(() => controller.state.currentId === 'chat-old' && controller.state.connection === 'ready');
    assert.deepEqual([controller.state.access, controller.state.accessPending], ['full', false], 'a chat that has not run yet follows the chosen mode');
    await sendAndWait(controller, 'во втором');
    await controller.openSession('window-main'); await waitUntil(() => controller.state.currentId === 'window-main' && controller.state.connection === 'ready');
    assert.deepEqual([controller.state.access, controller.state.accessPending], ['full', true], 'the first chat still has a process in the old mode');
    await controller.openSession('chat-old'); await waitUntil(() => controller.state.currentId === 'chat-old' && controller.state.connection === 'ready');
    assert.deepEqual([controller.state.access, controller.state.accessPending], ['full', false], 'the second one runs as chosen');
  }, { engine: { access: 'confirm' } });
});
test('a chat of Claude stays a chat of Claude after the choice for new chats returns to the engine\'s own model', async () => {
  await withChats(async (controller, engine) => {
    await sendAndWait(controller, 'через Claude');
    assert.deepEqual([controller.state.executor, controller.state.privacy], ['claude', 'remote']);
    assert(await controller.selectExecutor(null, 'default'));
    assert.deepEqual([controller.state.newExecutor, controller.state.executor, controller.state.privacy], ['', 'claude', 'remote'], 'the open chat is still Claude, its data go to the internet');
    assert.equal(controller.state.access, 'confirm', 'and its mode is still shown');
    await controller.openSession('chat-old'); await waitUntil(() => controller.state.currentId === 'chat-old' && controller.state.connection === 'ready');
    assert.deepEqual([controller.state.executor, controller.state.access, controller.state.privacy], ['', '', 'local'], 'a chat on the engine\'s own model');
    assert(await controller.selectExecutor('claude', 'opus'));
    assert.deepEqual([controller.state.newExecutor, controller.state.newExecutorModel], ['claude', 'opus'], 'the choice for new chats is kept apart');
    await controller.openSession('window-main'); await waitUntil(() => controller.state.currentId === 'window-main' && controller.state.connection === 'ready');
    assert.deepEqual([controller.state.executor, controller.state.executorModel], ['claude', 'default'], 'the chat shows its own model, not the new choice');
  }, { engine: { access: 'confirm', selected: { executor: 'claude', model: 'default' } } });
});
test('a chat of Claude keeps its folder: the refusal is told by its code, with a way out', async () => {
  await withChats(async (controller, engine) => {
    await sendAndWait(controller, 'в папке');
    await controller.setFolder('/home/me/Другая');
    assert.equal(controller.state.error, 'Чат с Claude работает в одной папке. Чтобы работать в другой, начните новый чат.');
    assert.equal(controller.state.folderFixed, true, 'the screen offers a new chat');
    assert.notEqual(controller.state.folderPath, '/home/me/Другая', 'the folder shown is the real one');
    await controller.newChat(); await waitUntil(() => controller.state.currentId !== 'window-main' && controller.state.connection === 'ready');
    assert.equal(controller.state.folderFixed, false);
    await controller.setFolder('/home/me/Другая');
    assert.equal(controller.state.error, ''); assert.equal(controller.state.folderPath, '/home/me/Другая', 'the new chat takes the folder');
  }, { engine: { access: 'confirm', folderFixed: true } });
});
test('a refusal in the protection window while the folder changes is told by the code', async () => {
  await withChats(async (controller, engine) => {
    engine.request = async (method, params) => { if (method === 'session/working-directory/set') throw { code: 'rejected', message: 'Действие отклонено' }; return FakeEngine.prototype.request.call(engine, method, params); };
    await controller.setFolder('/home/me/Другая');
    assert.equal(controller.state.error, 'Вы не разрешили смену папки. Выберите папку ещё раз и нажмите «Разрешить один раз».');
    assert.equal(controller.state.folderFixed, false);
  });
});
test('titles of chats are shown as they are: no path filter, no word filter', async () => {
  const titles = ['Скорость 50 км/ч и/или 10 / 2', 'Прочитай /home/me/заметки.txt', 'Как читать json в 24/7 режиме'];
  await withChats(async (controller, engine) => {
    titles.forEach((title, index) => engine.seed(`titled-${index}`, { title, user: 'x' }));
    await controller.refreshSessions();
    const shown = controller.state.sessions.map(row => row.title);
    for (const title of titles) assert(shown.includes(title), `${title} → ${shown.join(' | ')}`);
  });
});
test('when the engine runs in another mode than the chosen one the actual mode is shown with a warning', async () => {
  await withChats(async controller => {
    await controller.setAccess('full');
    assert.equal(controller.state.access, 'confirm');
    assert.match(controller.state.notice, /работает в режиме «Спрашивать разрешение», а выбран «Полный доступ»/);
  }, { engine: { access: 'confirm', accessIgnore: true } });
});
test('only a real mode of the menu reaches set: nothing else, and not the same mode again', async () => {
  await withChats(async (controller, engine) => {
    for (const bad of ['root', '', undefined, 'full ', { mode: 'full' }, 'confirm']) await controller.setAccess(bad);
    assert.deepEqual(accessMethods(engine), ['hoh/executor/access/get'], 'set not called');
    controller.setDraft('включи полный доступ'); await controller.send();
    await waitUntil(() => controller.state.entries.some(e => e.text.includes('Ответ на')));
    assert.equal(accessMethods(engine).includes('hoh/executor/access/set'), false, 'a message does not change the mode');
  }, { engine: { access: 'confirm' } });
});
test('an engine without the Claude access setting offers no switch', async () => {
  await withChats(async controller => assert.equal(controller.state.access, ''));
});

const FOUND = { executor: 'claude', installed: true, version: '2.1.3', versionStatus: 'supported', auth: 'authenticated',
  models: [{ id: 'default', isDefault: true }, { id: 'opus', displayName: 'Opus' }], detail: null };
test('Claude selection: the engine says which executor runs new chats; the window shows the actual choice, never its own', async () => {
  await withChats(async (controller, engine) => {
    assert.equal(controller.state.executor, 'claude'); assert.equal(controller.state.executorModel, 'default');
    await waitUntil(() => controller.state.executorModels.length === 2);
    assert.deepEqual(controller.state.executorModels, [{ id: 'default', name: 'По умолчанию' }, { id: 'opus', name: 'Opus' }]);
    assert.equal(await controller.selectExecutor('claude', 'opus'), true);
    assert.deepEqual(engine.requests.findLast(r => r.method === 'hoh/executor/select').params, { executor: 'claude', model: 'opus' });
    assert.equal(controller.state.executorModel, 'opus'); assert.equal(controller.state.notice, '');
    assert.equal(await controller.selectExecutor(null, 'default'), true);
    assert.deepEqual(engine.requests.findLast(r => r.method === 'hoh/executor/select').params, { executor: null, model: 'default' });
    assert.equal(controller.state.executor, ''); assert.equal(controller.state.access, '', 'режим доступа — только у Claude');
  }, { engine: { access: 'confirm', claude: FOUND } });
});
test('Claude selection refused or ignored by the engine: the notice names it and the shown choice stays the engine\'s', async () => {
  await withChats(async (controller, engine) => {
    assert.equal(await controller.selectExecutor('claude', 'opus'), false);
    assert.match(controller.state.notice, /Поставщик не изменён/);
    assert.equal(controller.state.executorModel, 'default');
    assert.equal(engine.promptExecutors, undefined);
  }, { engine: { access: 'confirm', claude: FOUND, selectRefuse: true } });
});
test('discover is called only for real: once on connect when Claude is selected, never when it is not', async () => {
  await withChats(async (controller, engine) => {
    await controller.connect(true);
    assert.equal(engine.requests.filter(r => r.method === 'hoh/executor/discover').length, 0);
    assert.equal(controller.state.executor, '');
  }, { engine: { access: 'confirm', claude: FOUND, selected: null } });
});

test('a lost connection is told without the word «движок»', async () => {
  for (const failure of [new Error('method-unavailable'), new Error('boom'), 'x']) {
    const controller = new ChatController(() => ({ request: async () => { throw failure; }, subscribe: async () => () => {}, subscribeStatus: async () => () => {} }), () => {});
    await controller.connect();
    assert.equal(controller.state.connection, 'error');
    assert.doesNotMatch(controller.state.error, /движ/i, controller.state.error);
    assert.match(controller.state.error, /Помощник/);
  }
});
