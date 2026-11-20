import assert from 'node:assert/strict';
import { test } from 'node:test';
import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';

const lex = text => unified().use(remarkParse).use(remarkGfm).parse(text).children;
import { harmfulMarkdownCases } from './markdown-cases.mjs';
import { ChatProjection, assistantChunks, humanFile, numberInWords, visibleText } from '../../src/screens/chat/projection.mjs';
import { StubClient, recordedPrompt, WORKSPACE } from './stub-client.mjs';

function event(type, seq, data) { return { type, seq, time: 0, data }; }
async function replay(scenario) {
  const stub = await StubClient.open(scenario);
  try {
    await stub.request('session/resume', { sessionId: 'session-1' });
    await stub.request('session/prompt', { sessionId: 'session-1', contentBlocks: [{ type: 'text', text: await recordedPrompt(scenario) }] });
    await stub.request('session/wait', { sessionId: 'session-1' });
    return stub.notifications.filter(n => n.method === 'session.event').map(n => n.params.event);
  } finally { await stub.close(); }
}

test('text-turn comes through the actual replay process, preserving its text chunks', async () => {
  const events = await replay('text-turn');
  const projection = new ChatProjection(WORKSPACE);
  for (const row of events) projection.event(row);
  const assistant = projection.entries.filter(e => e.kind === 'assistant');
  assert.equal(assistant.length, 1); assert.equal(assistant[0].text, 'SDK snapshot OK');
  assert.equal(projection.running, false);
  assert.deepEqual(assistantChunks(events.find(e => e.type === 'assistant/message')), ['SD', 'K', ' snapshot', ' OK']);
  assert(!JSON.stringify(projection.entries).includes('reasoning'));
});

test('persistent-tools becomes read/write/command rows with real recorded results', async () => {
  const rows = await replay('persistent-tools');
  const projection = new ChatProjection(WORKSPACE);
  rows.forEach(row => projection.event(row));
  const steps = projection.entries.filter(e => e.kind === 'step');
  assert.equal(steps.length, 7);
  assert.equal(steps.filter(e => e.operation === 'command').length, 4);
  assert.equal(steps.filter(e => e.operation === 'write').length, 2);
  assert.equal(steps.filter(e => e.operation === 'read').length, 1);
  assert(steps.some(e => e.label === 'Прочитал note.txt' && e.detail.includes('target:') && e.detail.includes('old')));
  assert(steps.some(e => e.label === 'Записал note.txt' && e.detail.includes('target:')));
  assert(steps.some(e => e.label === 'Выполнено' && e.detail === 'delayed'));
  assert(steps.some(e => e.status === 'error' && e.exitCode === 9 && e.label === 'Не удалось выполнить' && !/код/.test(e.label)));
  for (const step of steps) {
    assert(!step.label.includes('{') && !step.label.includes('bash') && !step.label.includes('str_replace_editor'));
    assert(!/[\\/]workspace|[\\/]tmp|инструмент|токен|сесси/i.test(step.label + step.detail));
  }
  assert.equal(projection.entries.filter(e => e.kind === 'assistant').at(-1).text, 'PERSISTENT_TOOLS_OK');
});

test('RPC session/cancel is accepted by the actual stub', async () => {
  const stub = await StubClient.open();
  try {
    await stub.request('session/resume', { sessionId: 'session-1' });
    assert.deepEqual(await stub.request('session/cancel', { sessionId: 'session-1' }), {});
    assert.equal(stub.requests.filter(r => r.method === 'session/cancel').length, 1);
  } finally { await stub.close(); }
});

test('history/live duplicate seq and user receipt do not duplicate rows', () => {
  const p = new ChatProjection(WORKSPACE);
  const local = p.optimistic('Привет');
  const row = event('user/message', 1, { id: 'u1', content: [{ type: 'text', text: 'Привет' }], source: { kind: 'user' } });
  p.event(row); p.receipt(local, 'u1'); p.event(row);
  assert.equal(p.entries.length, 1); assert.equal(p.entries[0].pending, false);
  p.event(event('user/message', 2, { id: 'u1', content: [{ type: 'text', text: 'Привет' }], source: { kind: 'user' } }));
  assert.equal(p.entries.length, 1);
});

test('system reminders, reasoning, usage and internal calls never become messages', () => {
  const p = new ChatProjection();
  p.event(event('system/message', 0, { message: { content: [{ type: 'text', text: 'secret' }] } }));
  p.event(event('user/message', 1, { id: 'hidden', source: { kind: 'runtime-context' }, content: [{ type: 'text', text: 'secret' }] }));
  p.event(event('assistant/message', 2, { message: { id: 'a', content: [{ type: 'reasoning', text: 'secret' }, { type: 'tool-call', arguments: '{"secret":true}' }] } }));
  assert.deepEqual(p.entries, []);
});

test('outside paths and traversal never appear, including in result text', () => {
  assert.equal(humanFile('/workspace/chat/note.txt', WORKSPACE), 'note.txt');
  assert.equal(humanFile('/workspace/chat/../private/key', WORKSPACE), 'файл');
  assert.equal(humanFile('/workspace/chatty/private', WORKSPACE), 'файл');
  assert.equal(humanFile('../../secret', WORKSPACE), 'файл');
  assert.equal(humanFile('C:\\Users\\Someone\\private.txt', WORKSPACE), 'файл');
  assert.equal(visibleText('Ошибка в /etc/passwd и C:\\Users\\Someone\\private.txt', WORKSPACE), 'Ошибка в файл и файл');
  assert.equal(visibleText('<script>alert(1)</script>', WORKSPACE), '<script>alert(1)</script>');
});

test('malformed arguments and unknown calls get generic human labels, never JSON', () => {
  const p = new ChatProjection();
  p.event(event('tool/call', 0, { callId: 'x', name: 'internal_secret_name', arguments: '{bad json' }));
  p.event(event('tool/result', 1, { message: { toolCallId: 'x', content: [{ type: 'text', text: '{"internal":"secret"}' }], isError: false } }));
  assert.equal(p.entries[0].label, 'Готово'); assert.equal(p.entries[0].detail, '');
});

test('stop freezes partial text and marks unfinished work as stopped', () => {
  const p = new ChatProjection();
  const assistant = event('assistant/message', 2, { message: { id: 'a', content: [{ type: 'text', text: 'Частичный ответ' }] } });
  p.event(event('turn/start', 0, {}));
  p.event(event('tool/call', 1, { name: 'read', callId: 'r', arguments: '{"file_path":"note.txt"}' }));
  p.assistant(assistant, 'Частичный', true); p.stop();
  assert.equal(p.running, false); assert.equal(p.entries[0].status, 'stopped');
  assert.equal(p.entries[1].text, 'Частичный'); assert.equal(p.entries[1].streaming, false);
});

test('updating one row preserves the identity of every other row', () => {
  const p = new ChatProjection();
  p.event(event('user/message', 0, { id: 'u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Привет' }] }));
  const first = p.entries[0]; const snapshot = p.entries;
  const row = event('assistant/message', 1, { message: { id: 'a' } });
  p.assistant(row, 'Часть', true); p.assistant(row, 'Часть ответа', false);
  assert.equal(p.entries[0], first); assert.equal(snapshot.length, 1); assert.equal(p.entries.length, 2);
});

test('exit code words and structured command output are human readable', () => {
  assert.equal(numberInWords(0), 'ноль'); assert.equal(numberInWords(9), 'девять'); assert.equal(numberInWords(255), 'двести пятьдесят пять');
  const p = new ChatProjection();
  p.event(event('tool/call', 0, { name: 'bash', callId: 'b', arguments: '{}' }));
  p.event(event('tool/result', 1, { message: { toolCallId: 'b', content: [{ type: 'text', text: '{"stdout":{"text":"готово"},"exitCode":0}' }] } }));
  assert.equal(p.entries[0].detail, 'готово'); assert.equal(p.entries[0].label, 'Выполнено');
});

test('5000 projected messages remain distinct and reachable', () => {
  const p = new ChatProjection();
  for (let i = 0; i < 5000; i++) p.event(event('user/message', i, { id: 'u-' + i, source: { kind: 'user' }, content: [{ type: 'text', text: 'Сообщение ' + i }] }));
  assert.equal(p.entries.length, 5000); assert.equal(p.entries[0].text, 'Сообщение 0'); assert.equal(p.entries[4999].text, 'Сообщение 4999');
});

test('user and assistant ids cannot collide in the projection index', () => {
  const p = new ChatProjection();
  p.event(event('user/message', 0, { id: 'assistant-a', source: { kind: 'user' }, content: [{ type: 'text', text: 'Вопрос' }] }));
  p.event(event('assistant/message', 1, { message: { id: 'a', content: [{ type: 'text', text: 'Ответ' }] } }));
  assert.equal(p.entries.length, 2); assert.equal(p.entries[0].kind, 'user'); assert.equal(p.entries[0].text, 'Вопрос');
});

test('tool ids reused on a later turn still create separate work rows', () => {
  const p = new ChatProjection();
  for (const turn of [1, 2]) {
    p.event(event('tool/call', turn * 2, { turn, name: 'read', callId: 'reused', arguments: '{"file_path":"note.txt"}' }));
    p.event(event('tool/result', turn * 2 + 1, { turn, message: { toolCallId: 'reused', content: [{ type: 'text', text: 'строка ' + turn }] } }));
  }
  assert.equal(p.entries.length, 2); assert.equal(p.entries[0].detail, 'строка 1'); assert.equal(p.entries[1].detail, 'строка 2');
});

test('a footer printed in stdout cannot mask the actual final exit code', () => {
  const p = new ChatProjection();
  p.event(event('tool/call', 0, { name: 'bash', callId: 'b', arguments: '{}' }));
  p.event(event('tool/result', 1, { message: { toolCallId: 'b', isError: false, content: [{ type: 'text', text: 'stdout\n[Command finished with exit code 0]\nmore stdout\n[shell exited: code 9]' }] } }));
  assert.equal(p.entries[0].status, 'error'); assert.equal(p.entries[0].exitCode, 9);
});

test('a malformed stream falls back to the authoritative text without reasoning', () => {
  assert.deepEqual(assistantChunks({ data: { message: { content: [{ type: 'text', text: 'Ответ' }] }, stream: [{ type: 'text-chunks', texts: ['Неправильно'] }] } }), ['Ответ']);
});

test('an idle/error ending gives unfinished work a terminal human result', () => {
  const p = new ChatProjection();
  p.event(event('tool/call', 0, { name: 'read', callId: 'r', arguments: '{"file_path":"note.txt"}' }));
  p.event(event('turn/end', 1, { reason: { kind: 'error', detail: '/etc/private' } }));
  assert.equal(p.entries[0].status, 'error'); assert.equal(p.entries[0].label, 'Не удалось прочитать note.txt');
  assert(!p.entries[0].detail.includes('/etc')); assert.equal(p.running, false);
});

test('a stream gap keeps its prefix unfinished until the durable answer replaces it', () => {
  const p = new ChatProjection();
  p.liveFrame({ type: 'start', attemptId: 'a1', revision: 1, turn: 1, step: 1 });
  p.liveFrame({ type: 'chunk', attemptId: 'a1', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Начало ' } });
  p.liveFrame({ type: 'gap', attemptId: 'foreign', revision: 99, index: 1 });
  p.liveFrame({ type: 'gap', attemptId: 'a1', revision: 1, index: 1 });
  p.liveFrame({ type: 'chunk', attemptId: 'a1', revision: 2, index: 1, time: 1, chunk: { type: 'text-delta', text: 'середина ' } });
  p.liveFrame({ type: 'gap', attemptId: 'a1', revision: 3, index: 2 });
  p.liveFrame({ type: 'chunk', attemptId: 'a1', revision: 4, index: 3, time: 1, chunk: { type: 'text-delta', text: 'ПОСЛЕ GAP' } });
  p.liveFrame({ type: 'end', attemptId: 'a1', revision: 5, index: 4, outcome: 'committed' });
  p.liveFrame({ type: 'gap', attemptId: 'a1', revision: 6, index: 5 });
  assert.deepEqual(p.entries.map(entry => [entry.text, entry.streaming]), [['Начало середина ', true]]);

  p.event(event('assistant/message', 1, { turn: 1, step: 1, message: { id: 'final', content: [{ type: 'text', text: 'Начало и полный конец.' }] } }));
  assert.deepEqual(p.entries, [{ id: 'assistant-final', kind: 'assistant', text: 'Начало и полный конец.', streaming: false }]);
});

test('an incomplete marker is italic outside an unterminated Markdown fence', () => {
  const p = new ChatProjection();
  p.liveFrame({ type: 'start', attemptId: 'fenced', revision: 1, turn: 1, step: 1 });
  p.liveFrame({ type: 'chunk', attemptId: 'fenced', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Пример:\n```js\nalert(1)' } });
  p.liveFrame({ type: 'gap', attemptId: 'fenced', revision: 3, index: 1 });
  p.finishGaps();
  assert.equal(p.entries[0].text, 'Пример:\n```js\nalert(1)\n```\n\n*Ответ получен не полностью.*');
  const tokens = lex(p.entries[0].text);
  assert.equal(tokens.at(-1).type, 'paragraph');
  assert.equal(tokens.at(-1).children[0].type, 'emphasis');

  const crlf = new ChatProjection();
  const closed = 'Пример:\r\n```js\r\nalert(1)\r\n```\r\nПосле';
  crlf.liveFrame({ type: 'start', attemptId: 'crlf', revision: 1, turn: 2, step: 1 });
  crlf.liveFrame({ type: 'chunk', attemptId: 'crlf', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: closed } });
  crlf.liveFrame({ type: 'gap', attemptId: 'crlf', revision: 3, index: 1 });
  crlf.finishGaps();
  assert.equal(crlf.entries[0].text, closed + '\n\n*Ответ получен не полностью.*');
  const crlfTokens = lex(crlf.entries[0].text);
  assert.equal(crlfTokens.at(-1).children[0].type, 'emphasis');
});

test('an empty canonical message removes a gapped prefix like an ordinary empty message', () => {
  const ordinary = new ChatProjection();
  ordinary.event(event('assistant/message', 1, { turn: 1, step: 1, message: { id: 'empty-ordinary', content: [] } }));
  assert.deepEqual(ordinary.entries, []);

  const p = new ChatProjection();
  p.liveFrame({ type: 'start', attemptId: 'empty-gap', revision: 1, turn: 1, step: 1 });
  p.liveFrame({ type: 'chunk', attemptId: 'empty-gap', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Лишний префикс' } });
  p.liveFrame({ type: 'gap', attemptId: 'empty-gap', revision: 3, index: 1 });
  p.event(event('assistant/message', 2, { turn: 1, step: 1, message: { id: 'empty-gap-final', content: [] } }));
  assert.deepEqual(p.entries, []);
  assert.equal(p.hasGap(), false);

  const indexed = new ChatProjection();
  indexed.event(event('user/message', 3, { id: 'before', source: { kind: 'user' }, content: [{ type: 'text', text: 'До' }] }));
  indexed.liveFrame({ type: 'start', attemptId: 'indexed-gap', revision: 1, turn: 2, step: 1 });
  indexed.liveFrame({ type: 'chunk', attemptId: 'indexed-gap', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Лишнее' } });
  indexed.liveFrame({ type: 'gap', attemptId: 'indexed-gap', revision: 3, index: 1 });
  indexed.event(event('tool/call', 4, { turn: 2, name: 'read', callId: 'after', arguments: { file_path: 'note.txt' } }));
  indexed.event(event('assistant/message', 5, { turn: 2, step: 1, message: { id: 'empty-indexed', content: [] } }));
  indexed.event(event('tool/result', 6, { turn: 2, message: { toolCallId: 'after', content: [{ type: 'text', text: 'После' }] } }));
  indexed.event(event('user/message', 7, { id: 'before', source: { kind: 'user' }, content: [{ type: 'text', text: 'До' }] }));
  assert.deepEqual(indexed.entries.map(entry => entry.kind === 'step' ? [entry.kind, entry.detail] : [entry.kind, entry.text]), [['user', 'До'], ['step', 'После']]);

  const late = new ChatProjection();
  late.event(event('user/message', 8, { id: 'kept', source: { kind: 'user' }, content: [{ type: 'text', text: 'До' }] }));
  late.liveFrame({ type: 'start', attemptId: 'late-indexed', revision: 1, turn: 3, step: 1 });
  late.liveFrame({ type: 'chunk', attemptId: 'late-indexed', revision: 2, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Неполно' } });
  late.liveFrame({ type: 'gap', attemptId: 'late-indexed', revision: 3, index: 1 });
  late.finishGaps();
  late.event(event('tool/call', 9, { turn: 3, name: 'read', callId: 'late-after', arguments: { file_path: 'note.txt' } }));
  late.event(event('assistant/message', 10, { turn: 3, step: 1, message: { id: 'late-empty-indexed', content: [] } }));
  late.event(event('tool/result', 11, { turn: 3, message: { toolCallId: 'late-after', content: [{ type: 'text', text: 'Индекс сдвинут' }] } }));
  assert.deepEqual(late.entries.map(entry => entry.kind === 'step' ? [entry.kind, entry.detail] : [entry.kind, entry.text]), [['user', 'До'], ['step', 'Индекс сдвинут']]);
});

test('a JSON-shaped or HTML-shaped filename stays a generic file label', () => {
  assert.equal(humanFile('{"name":"secret"}', ''), 'файл');
  assert.equal(humanFile('<img onerror=alert(1)>', ''), 'файл');
  assert.equal(humanFile('отчёт.txt', ''), 'отчёт.txt');
});

test('outside paths are hidden in every wrapper in assistant text and window details', async () => {
  const { pathCases } = await import('./privacy-cases.mjs');
  for (const [source, expected] of pathCases) {
    assert.equal(visibleText(source, WORKSPACE), expected, source);
    const p = new ChatProjection(WORKSPACE);
    p.event(event('assistant/message', 0, { message: { id: 'a', content: [{ type: 'text', text: source }] } }));
    assert.equal(p.entries[0].text, expected, source);
    p.event(event('tool/call', 1, { name: 'bash', callId: 'r', arguments: '{}' }));
    p.event(event('tool/result', 2, { message: { toolCallId: 'r', content: [{ type: 'text', text: source }] } }));
    assert(!p.entries[1].detail.includes('/etc') && !p.entries[1].detail.includes('secret.txt'), source);
  }
});

test('technical output and JSON stay out of UI labels/details; assistant prose is untouched', async () => {
  const { technicalCases } = await import('./privacy-cases.mjs');
  for (const source of technicalCases) {
    const p = new ChatProjection(WORKSPACE);
    p.event(event('assistant/message', 0, { message: { id: 'a', content: [{ type: 'text', text: source }] } }));
    assert.equal(p.entries[0].text, source);
    for (const [index, name] of ['bash', 'write_file', 'read_file', 'unknown'].entries()) {
      const seq = index * 2 + 1;
      p.event(event('tool/call', seq, { name, callId: name, arguments: { path: '/workspace/chat/agent.json', content: source } }));
      p.event(event('tool/result', seq + 1, { message: { toolCallId: name, content: [{ type: 'text', text: source }] } }));
      const row = p.entries.at(-1);
      assert(!/агент|сесси|токен|инструмент|agent|session|token|tool|bash|read_file|str_replace_editor|\{|\[/i.test(row.label + row.detail), source);
      assert.equal(row.file, 'файл');
    }
  }
});

test('history retains acknowledged inbox input before a turn or user echo exists', () => {
  const p = new ChatProjection();
  const data = { id: 'acknowledged', source: { kind: 'user' }, content: [{ type: 'text', text: 'Вопрос' }] };
  p.event(event('agent/inbox/spliced', 0, { inserted: [data] }));
  p.event(event('user/message', 1, data));
  assert.equal(p.entries.length, 1); assert.equal(p.entries[0].text, 'Вопрос');
});


test('an unfamiliar English tool name is hidden in its own generated result', () => {
  const p = new ChatProjection();
  p.event(event('tool/call', 0, { name: 'special_internal_reader', callId: 'x', arguments: {} }));
  p.event(event('tool/result', 1, { message: { toolCallId: 'x', content: [{ type: 'text', text: 'SPECIAL_INTERNAL_READER completed' }] } }));
  assert.equal(p.entries[0].detail, 'Содержимое результата не показывается.');
});


test('path privacy never depends on the preceding punctuation or letter', () => {
  const prefixes = [...new Set([...`!"#$%&'()*+,-.:;<=>?@[]^_\`{|}~«»“”„…—–。、：，；（）【】`, 'Ошибка'])];
  const paths = ['/etc/passwd', String.raw`C:\Users\Someone\secret.txt`, String.raw`\\server\share\secret.txt`, 'file:///etc/passwd', 'file://server/share/secret.txt'];
  for (const prefix of prefixes) for (const path of paths) {
    const source = prefix + path;
    const p = new ChatProjection(WORKSPACE);
    p.event(event('assistant/message', 0, { message: { id: 'a', content: [{ type: 'text', text: source }] } }));
    assert.equal(p.entries[0].text, prefix + 'файл', source);
    p.event(event('tool/call', 1, { name: 'bash', callId: 'r', arguments: {} }));
    p.event(event('tool/result', 2, { message: { toolCallId: 'r', content: [{ type: 'text', text: source }] } }));
    assert(!/etc|Users|Someone|server|share|secret/.test(p.entries[1].detail), source);
  }
});

test('paired literal HTML stays text while its external file attributes are hidden', () => {
  const source = '<iframe srcdoc="<script>alert(1)</script>" src="file:///etc/passwd"></iframe>';
  assert.equal(visibleText(source, WORKSPACE), '<iframe srcdoc="<script>alert(1)</script>" src="файл"></iframe>');
  assert.equal(visibleText('<script>Ошибка:</etc/passwd></script>', WORKSPACE), '<script>Ошибка:<файл></script>');
});


for (const [index, { source, expected }] of harmfulMarkdownCases.entries()) {
  test('harmful Markdown literal source ' + (index + 1) + ' changes only external file paths', () => {
    assert.equal(visibleText(source, WORKSPACE), expected, source);
  });
}

test('data URI MIME headers are not paths, while external paths in their payload stay private', () => {
  for (const [source, expected] of [
    ['data:image/svg+xml,<svg>', 'data:image/svg+xml,<svg>'],
    ['DATA:IMAGE/SVG+XML;base64,PHN2Zz4=', 'DATA:IMAGE/SVG+XML;base64,PHN2Zz4='],
    ['data:text/plain;charset=utf-8,готово', 'data:text/plain;charset=utf-8,готово'],
    ['data:image/svg+xml,/etc/passwd', 'data:image/svg+xml,файл'],
    ['data:text/plain,file:///etc/passwd', 'data:text/plain,файл'],
    ['data:image/svg+xml,<svg src="/etc/passwd">', 'data:image/svg+xml,<svg src="файл">'],
    ['data:,/etc/passwd', 'data:,файл'],
  ]) assert.equal(visibleText(source, WORKSPACE), expected, source);
});

test('a sandbox refusal is recognised by the engine marker and explained with the folder name and the way out', () => {
  const projection = new ChatProjection(); projection.folder = '/home/me/Проекты/сайт';
  let seq = 0;
  const call = (id, name, args) => projection.event(event('tool/call', seq++, { callId: id, name, arguments: args }));
  const result = (id, text, isError = true) => projection.event(event('tool/result', seq++, { message: { toolCallId: id, isError, content: [{ type: 'text', text }] } }));
  call('a', 'write_file', { path: '../x.txt', content: 'x' });
  result('a', '[sandbox: file access denied under workspace-write mode]\n[sandbox: escalation available — retry]');
  call('b', 'write_file', { path: 'y.txt', content: 'y' });
  result('b', 'something else broke');
  call('c', 'write_file', { path: '../z.txt', content: 'z' });
  result('c', '[sandbox: file access denied under read-only mode]');
  const [denied, other, readOnly] = projection.entries;
  assert.equal(denied.label, 'Запись запрещена за пределами папки');
  assert.match(denied.detail, /только в папке «сайт».*выберите другую папку внизу поля ввода/);
  assert.equal(other.label, 'Не удалось записать y.txt'); assert.match(other.detail, /Попробуйте ещё раз/);
  assert.match(readOnly.detail, /только читать/);
});

test('a read step shows the file text, not the engine markup around it', () => {
  const projection = new ChatProjection(); projection.folder = '/work';
  projection.event(event('tool/call', 10, { callId: 'r', name: 'read_file', arguments: { path: 'n.txt' } }));
  projection.event(event('tool/result', 11, { message: { toolCallId: 'r', content: [{ type: 'text', text: '<path>/work/n.txt</path>\n<type>file</type>\n<content>\n1: alpha\n2: beta\n\n(End of file - total 2 lines)\n</content>' }] } }));
  assert.equal(projection.entries[0].detail, 'alpha\nbeta');
});

test('a relative path into a sub-folder keeps its folder part; absolute and escaping paths are still hidden', () => {
  assert.equal(visibleText('[внутри](sub/inner.txt)', '/work'), '[внутри](sub/inner.txt)');
  assert.equal(visibleText('см. src/a.ts:12 и docs/x/y.md', '/work'), 'см. src/a.ts:12 и docs/x/y.md');
  assert.equal(visibleText('[x](/etc/passwd)', '/work'), '[x](файл)');
  assert.equal(visibleText('[x](sub/../../etc/passwd)', '/work'), '[x](subфайл)');
  assert.equal(visibleText('[x](../out.txt)', '/work'), '[x](файл)');
  assert.equal(visibleText('Ошибка:/etc/passwd', '/work'), 'Ошибка:файл');
});

test('a finished command is told apart by what it does, from its first word only; arguments never appear', () => {
  const projection = new ChatProjection(); projection.folder = '/work';
  let seq = 100;
  const run = (id, command) => {
    projection.event(event('tool/call', seq++, { callId: id, name: 'bash', arguments: { command } }));
    projection.event(event('tool/result', seq++, { message: { toolCallId: id, content: [{ type: 'text', text: 'ok' }] } }));
  };
  run('a', 'ls -la'); run('b', 'rm secret-plan.txt'); run('c', 'curl https://x.example/y'); run('d', '/bin/cat notes.txt');
  const labels = projection.entries.map(e => e.label);
  assert.deepEqual(labels, ['Выполнено: список файлов', 'Выполнено: удаление', 'Выполнено', 'Выполнено: чтение файла']);
  assert(!labels.join(' ').includes('secret'));
});

import { numberRepeatedSteps } from '../../src/screens/chat/projection.mjs';
test('steps right after each other that say the same get numbers, so no two headings in a row look alike', () => {
  const step = (id, label) => ({ id, kind: 'step', label });
  const out = numberRepeatedSteps([step('a', 'Выполнено'), step('b', 'Выполнено'), step('c', 'Выполнено'), { id: 'm', kind: 'assistant', text: 'x' }, step('d', 'Выполнено')]);
  assert.deepEqual(out.map(e => e.label ?? e.text), ['Выполнено', 'Выполнено (2)', 'Выполнено (3)', 'x', 'Выполнено']);
  const same = [step('a', 'Читаю'), step('b', 'Выполнено')];
  assert.equal(numberRepeatedSteps(same)[0], same[0], 'неповторяющиеся шаги остаются тем же объектом');
});
test('no step heading uses the words «команда» or «код»', () => {
  for (const operation of ['read', 'write', 'command', 'action']) for (const status of ['running', 'done', 'error', 'stopped']) {
    const p = new ChatProjection();
    p.apply({ type: 'tool/call', data: { callId: 'c', turn: 1, name: operation === 'command' ? 'bash' : operation === 'read' ? 'read' : operation === 'write' ? 'write_file' : 'other', arguments: JSON.stringify({ command: 'zzz', file_path: 'a.txt' }) } });
    const label = p.entries[0].label;
    assert.doesNotMatch(label, /команд|код/i, `${operation}/${status}: ${label}`);
  }
});
