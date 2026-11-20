import assert from 'node:assert/strict';
import { test } from 'node:test';
import { DEFAULT_ACCESS_MODE, claudeModelLabel, discoverView } from '../../src/lib/executor.mjs';

const base = { executor: 'claude', installed: true, version: '2.1.3', versionStatus: 'supported', auth: 'authenticated',
  models: [{ id: 'default', isDefault: true }, { id: 'opus', displayName: 'Opus' }], detail: null };

test('the default access mode is one constant: confirm', () => assert.equal(DEFAULT_ACCESS_MODE, 'confirm'));
test('found and signed in: ready, version in the sentence, models from discover, no hint or warning', () => {
  const view = discoverView(base);
  assert.equal(view.ready, true); assert.equal(view.tone, 'ok');
  assert.equal(view.headline, 'Готово. Ничего вводить не нужно.'); assert.equal(view.version, '2.1.3');
  assert.deepEqual(view.models, [{ id: 'default', name: 'По умолчанию' }, { id: 'opus', name: 'Opus' }]);
  assert.equal(view.defaultModel, 'default'); assert.equal(view.hint, ''); assert.equal(view.warning, '');
});
test('found, not signed in: not ready, told to open Claude and sign in', () => {
  const view = discoverView({ ...base, auth: 'unauthenticated', models: [] });
  assert.equal(view.ready, false);
  assert.equal(view.headline, 'Claude установлен, но в него не вошли. Откройте Claude, войдите и нажмите «Проверить ещё раз».');
});
test('not found: not ready, told to ask who set the computer up', () => {
  const view = discoverView({ ...base, installed: false, version: null, versionStatus: 'unknown', auth: 'unknown', models: [] });
  assert.equal(view.ready, false); assert.match(view.headline, /^Claude на этом компьютере не установлен\. Попросите того, кто настраивал вам компьютер/);
});
test('unknown or unsupported version: ready, but a warning is said in words', () => {
  for (const [status, pattern] of [['unknown', /Не удалось проверить, подходит ли эта версия/], ['unsupported', /не поддерживается/]]) {
    const view = discoverView({ ...base, versionStatus: status });
    assert.equal(view.ready, true); assert.equal(view.tone, 'warn'); assert.match(view.warning, pattern);
  }
});
test('sign-in state that is not known is not "ready"; garbage answers never throw', () => {
  assert.equal(discoverView({ ...base, auth: 'unknown' }).ready, false);
  for (const garbage of [null, undefined, {}, 'x', { installed: true, models: 'no' }]) assert.equal(discoverView(garbage).ready, false);
});
test('model names: displayName, then id; default is «По умолчанию»', () => {
  assert.equal(claudeModelLabel('opus', [{ id: 'opus', name: 'Opus' }]), 'Opus');
  assert.equal(claudeModelLabel('default'), 'По умолчанию'); assert.equal(claudeModelLabel('haiku'), 'haiku');
});
