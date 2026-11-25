import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resolve } from 'node:path';
import ts from 'typescript';
import { componentHarness, deferred, textOf } from './component-harness.mjs';
import { findLiterals, checkScreens } from './check-literals.mjs';
import { approvalRequest, deletionText } from './fixtures.mjs';
const ui = resolve(import.meta.dirname, '../..');
const SECRET = 'sk-node-test-secret';
const modelEngine = {
  MethodMissing: class extends Error {},
  loadModelSettings: async () => ({ model: 'saved' }),
  loadWorkFolder: async () => '/work',
  storeApiKey: async () => {},
  probeModel: async () => ({ ok: true, latencyMs: 20, toolCalls: true, models: ['first', 'second'] }),
  modelsFromProbe: result => result.models ?? [],
  saveModel: async () => {}, saveRules: async () => {}, pickFolder: async () => 'Документы/Проекты',
};
function firstRun(overrides = {}) {
  return componentHarness(resolve(ui, 'src/screens/firstrun/FirstRun.tsx'), 'FirstRun', { './engine': { ...modelEngine, ...overrides } }, { onFinish() {} });
}
async function approval({ expires = 105000, decide = async () => {}, humanText = deletionText } = {}) {
  const calls = [];
  const h = componentHarness(resolve(ui, 'src/screens/approval/ApprovalWindow.tsx'), 'ApprovalWindow', { './bridge': { approvalHost: () => ({
    current: async () => ({ approvalId: 'a', risk: 'high', humanText, expiresAt: expires }),
    decide: async (id, outcome) => { calls.push({ id, outcome }); await decide(id, outcome); },
  }) } });
  await h.settle(); return { h, calls };
}

test('failed key saving leaves the secret only in the uncontrolled input and can be retried', async () => {
  let fail = true; const keys = [], saving = deferred();
  const h = firstRun({ storeApiKey: async value => { keys.push(value); if (fail) await saving.promise; } });
  await h.settle(); await h.click('Сервис в интернете (по ключу)'); await h.input('fr-address', 'https://service.test/v1');
  await h.input('fr-key', SECRET);
  const check = h.button('Дальше'); check.props.onClick(); check.props.onClick(); await h.settle();
  assert.equal(keys.length, 1); assert(h.byId('fr-address').disabled); assert(h.byId('fr-key').disabled);
  assert(!h.slots.filter(slot => slot.kind === 'state').some(slot => JSON.stringify(slot.value)?.includes(SECRET)));
  saving.reject(new Error(SECRET)); await h.settle();
  assert(h.text().includes('Не удалось сохранить ключ'));
  assert(!h.text().includes(SECRET));
  assert(!h.slots.filter(slot => slot.kind === 'state').some(slot => JSON.stringify(slot.value)?.includes(SECRET)));
  assert.equal(h.inputs.get('fr-key').value, SECRET);
  assert.equal(h.byId('fr-key').props.value, undefined);
  fail = false; await h.click('Дальше');
  assert.deepEqual(keys, [SECRET, SECRET]); assert.equal(h.inputs.get('fr-key').value, '');
  assert(h.text().includes('Правила работы')); h.unmount();
});

test('probe selects the first returned model; manual override stays inside closed Advanced', async () => {
  const saves = [], probes = []; const h = firstRun({ saveModel: async (...args) => saves.push(args), probeModel: async (...args) => { probes.push(args); return { ok: true, latencyMs: 1, toolCalls: true, models: ['first', 'second'] }; } });
  await h.settle();
  const advanced = h.nodes().find(node => node.type === 'details');
  assert.equal(advanced.props.open, undefined);
  assert(textOf(advanced).includes('Название модели'));
  await h.click('Дальше');
  assert.equal(saves[0][1], 'first'); assert.equal(probes[0][1], '', 'автообнаружение не навязывает сохранённое имя');
  await h.click('Назад'); await h.input('fr-model', 'manual'); await h.click('Дальше');
  assert.equal(saves[1][1], 'manual'); assert.equal(probes[1][1], 'manual'); h.unmount();
});

test('an old probe response without models asks for Advanced when no saved model exists', async () => {
  const saves = [];
  const h = firstRun({ loadModelSettings: async () => null, probeModel: async () => ({ ok: true, latencyMs: 1, toolCalls: true }), saveModel: async (...args) => saves.push(args) });
  await h.click('Дальше'); assert(h.text().includes('Укажите название в «Дополнительно»'));
  assert(h.text().includes('Кто будет отвечать')); assert(!h.button('Дальше').disabled); assert.deepEqual(saves, []); h.unmount();
});

test('model and rules saves disable all conflicting fields and navigation until settled', async () => {
  const savingModel = deferred(), savingRules = deferred();
  const h = firstRun({ saveModel: () => savingModel.promise, saveRules: () => savingRules.promise });
  await h.settle(); await h.click('Сервис в интернете (по ключу)');
  await h.input('fr-address', 'https://service.test/v1'); await h.input('fr-key', SECRET);
  await h.click('Дальше');
  for (const id of ['fr-address', 'fr-model', 'fr-key']) assert(h.byId(id).disabled, id);
  assert(h.button('Назад').disabled); assert(h.button('Дальше').disabled);
  await h.click('Назад'); assert(h.text().includes('Кто будет отвечать'));
  savingModel.resolve(); await h.settle(); assert(h.text().includes('Правила работы'));
  await h.click('Открыть чат');
  for (const node of h.nodes().filter(node => ['input', 'button'].includes(node.type))) assert(node.disabled, textOf(node));
  savingRules.reject(new Error('Не сохранено')); await h.settle();
  assert(h.text().includes('Правила работы')); assert(!h.button('Назад').disabled); h.unmount();
});

test('already expired approvals render without action buttons on their first pending render', async () => {
  const { h, calls } = await approval({ expires: 99999 });
  assert.equal(h.button('Decline'), undefined); assert.equal(h.button('Allow once'), undefined);
  assert(h.text().includes('Confirmation expired')); await h.event('keydown', { key: 'Escape' });
  assert.deepEqual(calls, []); h.unmount();
});

test('decide checks the wall clock even between interval ticks', async () => {
  const { h, calls } = await approval({ expires: 102000 });
  await h.advance(1000); const allow = h.button('Allow once'); assert(!allow.disabled);
  await h.advance(1001, false); allow.props.onClick(); await h.settle();
  assert.deepEqual(calls, []); assert.equal(h.button('Allow once'), undefined); h.unmount();
});

for (const event of ['keydown', 'pagehide']) test('after a send failure ' + event + ' still rejects the pending request', async () => {
  let fail = true;
  const { h, calls } = await approval({ decide: async () => { if (fail) throw new Error('Could not send your answer.'); } });
  const listener = h.listeners(event)[0];
  await h.advance(1000); await h.click('Allow once'); assert(h.text().includes('Could not send your answer.'));
  assert.equal(h.listeners(event).length, 1);
  assert.equal(h.listeners(event)[0], listener, 'ошибка не пересоздаёт обработчик и не оставляет окно без отказа');
  fail = false; await h.event(event, { key: 'Escape' });
  assert.deepEqual(calls.map(call => call.outcome), ['allowed-once', 'rejected']);
  assert(h.text().includes('Declined'));
  assert.equal(h.listeners(event).length, 0, 'успешный отказ снимает обработчик ожидающего запроса');
  await h.event(event, { key: 'Escape' }); assert.equal(calls.length, 2); h.unmount();
});

test('deletion consequence is concrete and humanText remains one protocol-valid line', async () => {
  const { h } = await approval();
  assert(h.text().includes('The files will not go to the trash and cannot be restored.'));
  assert(h.text().includes(deletionText));
  const primary = approvalRequest();
  assert(!/[\p{Cc}\p{Cf}]/u.test(primary.humanText));
  assert(primary.humanText.length > 0 && primary.humanText.length <= 4096);
  assert(primary.humanText.includes('отчёт-2023.txt, черновик.docx, итог-старый.md'));
  assert.equal(h.nodes().filter(node => node.props.className === 'ap-details').length, 0);
  const unknown = h.exports.consequence({ risk: 'high', humanText: 'Неизвестное действие' });
  assert(unknown.text.includes('may be')); h.unmount();
});

test('literal guard rejects planted viewport dimensions and accepts the token', async () => {
  for (const unit of ['dvh', 'svh', 'lvh', 'vh', 'vw', 'dvw', 'svw', 'lvw', 'DVH']) assert.equal(findLiterals(`a { height: 100${unit}; }`).length, 1, unit);
  assert.equal(findLiterals('a { height: var(--viewport-height); }').length, 0);
  const result = await checkScreens(); assert.deepEqual(result.found, []);
});

test('window TypeScript still typechecks without emitting a build', () => {
  const config = ts.readConfigFile(resolve(ui, 'tsconfig.json'), ts.sys.readFile);
  const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, ui);
  const host = ts.createCompilerHost(parsed.options); host.getCurrentDirectory = () => ui;
  const program = ts.createProgram(parsed.fileNames, parsed.options, host);
  const errors = ts.getPreEmitDiagnostics(program);
  assert.equal(errors.length, 0, ts.formatDiagnosticsWithColorAndContext(errors, { getCurrentDirectory: () => ui, getCanonicalFileName: value => value, getNewLine: () => '\n' }));
});


test('model discovery compatibility is confined to the result adapter', () => {
  const h = componentHarness(resolve(ui, 'src/screens/firstrun/engine.ts'), 'modelsFromProbe', { '@tauri-apps/api/core': {} });
  assert.deepEqual(Array.from(h.exports.modelsFromProbe({ ok: true })), []);
  assert.deepEqual(Array.from(h.exports.modelsFromProbe({ models: [' first ', '', null, 'second'] })), ['first', 'second']);
  h.unmount();
});
