// Постоянная проверка слов (аудит ux-noncoder-audit, HOH-35): слова, которых человек без опыта кода не поймёт, не должны вернуться.
// Смотрит тексты окна (ui/src без перенесённого кода T3 и без комментариев) и тексты окна защиты (text.rs, только чтение).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';

const UI = resolve(import.meta.dirname, '../..');
const ROOT = resolve(UI, '..');
const FORBIDDEN = [/API-ключ/i, /\/v1/, /провайдер/i, /движок|движк/i, /песочниц/i, /Только чтение/i, /Команда завершена/i, /Неизвестная ошибка/i, /код /i];
/** The only places where a listed word is a value, not a text: model addresses and the exact local image-reference namespace pattern. */
const ALLOWED = [
  { file: 'screens/firstrun/FirstRun.tsx', line: /const LOCAL_ADDRESS = /},
  { file: 'lib/providers.mjs', line: /v1/ },
  { file: 'hoh-host/composer.ts', line: /^const IMAGE_CONTEXT_REFERENCE = / },
];

function files(dir) {
  return readdirSync(dir).flatMap(name => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return name === 't3' ? [] : files(path);
    return /\.(ts|tsx|mjs)$/.test(name) && !/\.d\.mts$/.test(name) ? [path] : [];
  });
}
function code(text) {
  return text.replace(/\/\*[\s\S]*?\*\//g, match => match.replace(/[^\n]/g, ' ')).split('\n').map(line => line.replace(/(^|\s)\/\/.*$/, '$1'));
}
function hits(text, source) {
  return code(text).flatMap((line, index) => FORBIDDEN.filter(word => word.test(line)).map(word => `${source}:${index + 1}: ${word} — ${line.trim().slice(0, 90)}`));
}

test('the words of the window do not use the forbidden words', () => {
  const found = files(join(UI, 'src')).flatMap(path => {
    const name = relative(join(UI, 'src'), path);
    const allowed = ALLOWED.filter(rule => rule.file === name);
    return code(readFileSync(path, 'utf8')).flatMap((line, index) => allowed.some(rule => rule.line.test(line)) ? []
      : FORBIDDEN.filter(word => word.test(line)).map(word => `ui/src/${name}:${index + 1}: ${word} — ${line.trim().slice(0, 90)}`));
  });
  assert.deepEqual(found, [], 'запрещённые слова в окне');
});

// Тексты окна защиты пишет o1 (crates/hoh-destructive-classifier/src/text.rs); здесь они только читаются.
const TEXT_RS = join(ROOT, 'crates/hoh-destructive-classifier/src/text.rs');
test('the words of the protection window (text.rs) do not use the forbidden words', () => {
  const found = hits(readFileSync(TEXT_RS, 'utf8'), 'text.rs');
  assert.deepEqual(found, [], 'запрещённые слова в тексте окна защиты — ждёт o1: text.rs');
});

// Тексты полного доступа: слова CEO дословно, границы «в папке Помощника» и «в своей папке» в них нет.
const FULL_WORDS = 'Помощник сам удаляет и меняет файлы в любых папках, куда есть доступ у вас, запускает программы и выходит в интернет, не спрашивая. Сделанное нельзя отменить.';
test('the full access texts are the exact words and never name a folder as the limit', async () => {
  const { FULL_ACCESS_DESCRIPTION } = await import('../../src/lib/executor.mjs');
  assert.equal(FULL_ACCESS_DESCRIPTION, FULL_WORDS);
  const ru = readFileSync(join(UI, 'src/t3/shims/ru.ts'), 'utf8');
  assert(ru.includes("'Allow commands and edits without prompts.': FULL_ACCESS_DESCRIPTION"), 'подпись в ru.ts берётся из одной константы');
  const dialog = readFileSync(join(UI, 'src/screens/chat/AccessMode.tsx'), 'utf8');
  assert(dialog.includes('{FULL_ACCESS_DESCRIPTION}'), 'окно включения берёт тот же текст');
  for (const text of [FULL_ACCESS_DESCRIPTION, dialog, ru]) assert.doesNotMatch(text, /в папке Помощника|в своей папке/);
});
test('the check itself sees a forbidden word', () => {
  assert.equal(hits('const a = "Только чтение";', 'x').length, 1);
  assert.equal(hits('// песочница\nconst a = 1;', 'x').length, 0, 'комментарий не считается');
  assert.equal(hits('const a = "http://x/v1";', 'x').length, 1, 'ссылка с двумя косыми не считается комментарием');
});
