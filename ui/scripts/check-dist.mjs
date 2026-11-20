import { readdir, readFile, stat } from 'node:fs/promises';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { gzipSync } from 'node:zlib';

async function files(dir) {
  return (await Promise.all((await readdir(dir)).map(async name => {
    const path = join(dir, name);
    return (await stat(path)).isDirectory() ? files(path) : [path];
  }))).flat();
}
const inertNames = new Set([
  'http://www.w3.org/2000/svg', 'http://www.w3.org/1998/Math/MathML',
  'http://www.w3.org/1999/xlink', 'http://www.w3.org/XML/1998/namespace',
  // Local model configuration is passed to the native host, never fetched by UI.
  'http://localhost:8000/v1',
  ...['resize', 'move', 'close-requested', 'destroyed', 'focus', 'blur', 'scale-change',
      'theme-changed', 'window-created', 'suspended', 'resumed', 'webview-created',
      'drag-enter', 'drag-over', 'drag-drop', 'drag-leave'].map(name => 'tauri://' + name),
]);
let total = 0;
let compressed = 0;
for (const path of await files('dist')) {
  const bytes = await readFile(path);
  if (!/\.(?:html|css|js)$/.test(path)) continue;
  total += bytes.length;
  const source = bytes.toString();
  // Every URL-like value is rejected except exact inert namespace/event names
  // and the first-run loopback configuration. No remote diagnostic URLs allowed.
  // Start from the fixed // delimiter. An optional greedy scheme prefix made
  // long URL-free identifiers quadratic; the bounded look-behind stays linear.
  const candidates = source.matchAll(/\/\/[A-Za-z0-9\[][^\s"'`<>\\)]*/g);
  for (const match of candidates) {
    const before = source.slice(Math.max(0, match.index - 64), match.index);
    const scheme = before.match(/([a-z][a-z0-9+.-]*:)$/i)?.[1] ?? '';
    const value = scheme + match[0];
    assert(inertNames.has(value), `external URL ${value} in ${path}`);
  }
  compressed += gzipSync(bytes).length;
  assert(bytes.length < 256 * 1024, `unexpectedly large individual text asset: ${path}`);
  assert(!/\beval\s*\(|\bnew\s+Function\s*\(/.test(source), `dynamic code in ${path}`);
}
// Package 1's empty-shell cap covered one screen; this build has chat, first run
// and approval entries. Keep both a whole-window cap and a compressed payload cap.
// 19.11 fix QA A: 512 -> 544 KiB for the «Подтверждения» section, live answer text and the approval details.
// 20.11 окно-как-T3: 544 -> 576 KiB: значки поставщиков из T3 (OpenAI, Claude, Anthropic, OpenRouter, Ollama) и стили чата/окна защиты; предел gzip не менялся.
assert(total < 576 * 1024, `window text assets unexpectedly large: ${total} bytes`);
// 20.11.2026, новый вид окна по design/DESIGN.md (боковая колонка, список чатов, токены системы, старая разметка и стили заменены):
// было 163468/163840, стало 163782/171971 (предел = факт + 5 %). Проверка остаётся включённой.
// 20.11.2026, строка поля ввода, ссылки/картинки/шаги в ответах: 165275/173538 (факт + 5 %), было 163782/171971.
// 20.11.2026, ссылки как кнопки, отказ вне папки, ключ: 167329/175695 (факт + 5 %), было 165275/173538.
// 20.11.2026, круг 2 (настоящий путь папки, фраза о ключе при вводе, затемнение узкой колонки): 167615/175996 (факт + 5 %), было 167329/175695.
// 20.11.2026, после установки npm-пакетов T3 (решение CEO): было 175924/175996, предел = факт + 5 % = 184720. Рендер T3 (+180 КБ) в сборку не входит, см. src/t3/render.tsx.
const GZIP_LIMIT = 184720;
assert(compressed < GZIP_LIMIT, `window compressed text assets unexpectedly large: ${compressed} bytes`);
console.log(`passed: dist has no external network URLs or dynamic code; text ${total}/589824, gzip ${compressed}/${GZIP_LIMIT} bytes`);
