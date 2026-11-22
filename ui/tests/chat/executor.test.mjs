import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { confirmDescription, PROJECT_SETTINGS_OFF_NOTE, projectSettingsIgnored, CONFIRM_ACCESS_DESCRIPTION, DEFAULT_ACCESS_MODE, claudeModelLabel, discoverView } from '../../src/lib/executor.mjs';

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
test('unknown version: a refusal like the engine\'s start (auth is unknown there too), not «Дальше»', () => {
  for (const auth of ['authenticated', 'unknown']) {
    const view = discoverView({ ...base, versionStatus: 'unknown', version: null, auth, models: [] });
    assert.equal(view.ready, false); assert.equal(view.tone, 'bad'); assert.equal(view.warning, '');
    assert.equal(view.headline, 'Не удалось определить версию Claude, поэтому работа с ним не начнётся. Нужна версия 2.x — обновите Claude и нажмите «Проверить ещё раз».');
  }
});
test('unsupported version: a refusal, not ready, exact words — also as the real engine answers it (auth unknown, no models)', () => {
  for (const real of [{}, { auth: 'unknown', models: [] }, { auth: 'unauthenticated' }]) {
    const view = discoverView({ ...base, versionStatus: 'unsupported', ...real });
    assert.equal(view.ready, false); assert.equal(view.tone, 'bad'); assert.equal(view.warning, '');
    assert.equal(view.headline, 'Эта версия Claude не поддерживается. Нужна версия 2.x — обновите Claude и нажмите «Проверить ещё раз».', 'the version is told before the sign-in');
  }
});
test('the caption of «Спрашивать разрешение» says the person\'s own Claude settings apply', () => {
  assert(readFileSync(new URL('../../src/t3/shims/ru.ts', import.meta.url), 'utf8').includes("'Ask before commands and file changes.': CONFIRM_ACCESS_LEAD"), 'ru.ts берёт первую фразу из константы');
  assert.equal(CONFIRM_ACCESS_DESCRIPTION, 'Помощник спрашивает перед изменениями. Действуют и ваши собственные настройки Claude: их разрешающие правила могут пропускать действия без вопроса.');
  const flags = value => ['--permission-mode', 'default', '--setting-sources', value, '--permission-prompt-tool', 'stdio'];
  assert.equal(confirmDescription(flags('user')), CONFIRM_ACCESS_DESCRIPTION, 'sources = user: both phrases');
  assert.equal(confirmDescription(flags('user,project,local')), CONFIRM_ACCESS_DESCRIPTION);
  assert.equal(confirmDescription(flags('')), 'Помощник спрашивает перед изменениями.', 'isolated: the first phrase only');
  assert.equal(confirmDescription(['--permission-mode', 'default']), 'Помощник спрашивает перед изменениями.', 'the engine did not say: the first phrase only');
  assert.equal(confirmDescription(undefined), 'Помощник спрашивает перед изменениями.');
});
test('project settings are off exactly when --setting-sources has no `project`', () => {
  assert.equal(PROJECT_SETTINGS_OFF_NOTE, 'Настройки Claude из папки проекта не применяются');
  const flags = value => ['--permission-mode', 'default', '--setting-sources', value];
  assert.equal(projectSettingsIgnored(flags('user')), true); assert.equal(projectSettingsIgnored(flags('')), true);
  assert.equal(projectSettingsIgnored(flags('user,project,local')), false); assert.equal(projectSettingsIgnored(flags('project')), false);
  assert.equal(projectSettingsIgnored(['--setting-sources=user']), true); assert.equal(projectSettingsIgnored(['--setting-sources=user, project']), false);
  assert.equal(projectSettingsIgnored(['--permission-mode', 'default']), false); assert.equal(projectSettingsIgnored(undefined), false);
});
test('sign-in state that is not known is not "ready"; garbage answers never throw', () => {
  assert.equal(discoverView({ ...base, auth: 'unknown' }).ready, false);
  for (const garbage of [null, undefined, {}, 'x', { installed: true, models: 'no' }]) assert.equal(discoverView(garbage).ready, false);
});
test('model names: displayName, then id; default is «По умолчанию»', () => {
  assert.equal(claudeModelLabel('opus', [{ id: 'opus', name: 'Opus' }]), 'Opus');
  assert.equal(claudeModelLabel('default'), 'По умолчанию'); assert.equal(claudeModelLabel('haiku'), 'haiku');
});
