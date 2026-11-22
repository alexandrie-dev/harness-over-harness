import assert from 'node:assert/strict';
import { test } from 'node:test';
import { errorText, hostError, hostWords, refusalWords } from '../../src/lib/hostError.mjs';

// What `engine_request` of the host rejects with (apps/hoh-desktop/src/errors.rs, asserted there in Rust): an object with a code.
const HOST = {
  rejected: { code: 'rejected', message: 'Действие отклонено' },
  expired: { code: 'expired', message: 'Подтверждение не получено' },
  folderFixed: { code: 'folder-fixed', message: 'Чат с Claude работает в одной папке. Чтобы работать в другой, начните новый чат' },
  refused: { code: 'refused', message: 'Помощник отклонил запрос' },
  unavailable: { code: 'unavailable', message: 'method-unavailable' },
};

test('the reason is read from the code of the host, in whichever form the rejection arrives', () => {
  for (const form of [value => value, value => JSON.stringify(value), value => Object.assign(new Error(value.message), { code: value.code })]) {
    assert.equal(hostError(form(HOST.rejected)).code, 'rejected');
    assert.equal(hostError(form(HOST.expired)).code, 'expired');
  }
  assert.equal(hostError(HOST.unavailable).code, 'unavailable');
});
test('a refusal is told by its code: the words of the host do not decide', () => {
  const words = { what: 'проверку', button: 'Дальше' };
  assert.equal(refusalWords(HOST.rejected, words), 'Вы не разрешили проверку. Нажмите «Дальше» и выберите «Разрешить один раз».');
  assert.equal(refusalWords(HOST.expired, words), 'Время на ответ вышло. Нажмите «Дальше» ещё раз.');
  // other words under the same code: the same text; the words of a refusal without the code say nothing
  assert.equal(refusalWords({ code: 'rejected', message: 'совсем другие слова' }, words), refusalWords(HOST.rejected, words));
  assert.equal(refusalWords({ message: 'Действие отклонено' }, words), '');
  assert.equal(refusalWords('Подтверждение отклонено', words), '');
});
test('older hosts without codes: only the two long-standing marks are recognised', () => {
  assert.equal(hostError('method-unavailable').code, 'unavailable');
  assert.equal(hostError(new Error('Помощник ещё подключается')).code, 'connecting');
  assert.equal(hostError('что-то').code, '');
});
test('the text for the window: by code, then the host\'s own Russian words, then the fallback; never an unknown error', () => {
  assert.match(errorText(HOST.folderFixed, 'x'), /^Чат с Claude работает в одной папке/);
  assert.equal(errorText(HOST.refused, 'x'), 'Помощник отклонил запрос');
  assert.equal(errorText({ code: 'failed', message: '' }, 'Не удалось.'), 'Не удалось.');
  assert.equal(errorText(undefined, 'Не удалось.'), 'Не удалось.');
  assert.equal(hostWords('boom: /etc/passwd'), '', 'чужая техническая строка не показывается');
});
