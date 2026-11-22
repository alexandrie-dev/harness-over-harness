import { readdir, readFile, stat } from 'node:fs/promises';
import { existsSync } from 'node:fs';
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
  // Public identifier of an HTML doctype in the parse5 quirks table (markdown pipeline); an identifier, never fetched.
  'http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd',
  'http://www.w3.org/1999/xhtml', 'http://www.w3.org/2000/xmlns/', // XML namespaces of the markdown pipeline (hast/parse5), never fetched
  'http://www.w3.org/1999/xlink', 'http://www.w3.org/XML/1998/namespace',
  // Local model configuration is passed to the native host, never fetched by UI.
  'http://localhost:8000/v1',
  ...['resize', 'move', 'close-requested', 'destroyed', 'focus', 'blur', 'scale-change',
      'theme-changed', 'window-created', 'suspended', 'resumed', 'webview-created',
      'drag-enter', 'drag-over', 'drag-drop', 'drag-leave'].map(name => 'tauri://' + name),
]);
// Вход окна на веб-клиенте T3 (HOH-40, `vite build --mode t3`): dist/index.html, dist/boot-theme.js, dist/t3-assets/**.
// Для него те же правила (ни одного внешнего адреса, кроме перечисленных; без eval/new Function), адреса из текста T3 —
// только из проверенного списка src/t3-entry/dist-urls.txt; пределы размера прежней оболочки к нему не относятся.
const t3Build = existsSync('dist/t3-assets');
const isT3 = path => t3Build && (path === join('dist', 'index.html') || path === join('dist', 'boot-theme.js') || path.startsWith(join('dist', 't3-assets') + '/'));
const t3Urls = new Set(existsSync('src/t3-entry/dist-urls.txt')
  ? (await readFile('src/t3-entry/dist-urls.txt', 'utf8')).split('\n').map(line => line.trim()).filter(line => line && !line.startsWith('#'))
  : []);
const t3Unknown = new Set();
let t3Total = 0;
let t3Compressed = 0;
let total = 0;
let compressed = 0;
for (const path of await files('dist')) {
  const bytes = await readFile(path);
  if (!/\.(?:html|css|js)$/.test(path)) continue;
  const t3 = isT3(path);
  if (!t3) total += bytes.length;
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
    // Without a scheme only a real host counts (host.tld, localhost, IPv4, [IPv6]); a regex tail such as //g,r=/ is code, not an address.
    if (!scheme && !/^\/\/(?:(?:[a-z0-9-]+\.)+[a-z]{2,}(?=[/:?#]|$)|localhost\b|\d{1,3}(?:\.\d{1,3}){3}\b|\[)/i.test(match[0])) continue;
    if (t3) { if (!inertNames.has(value) && !t3Urls.has(value)) t3Unknown.add(value); continue; }
    assert(inertNames.has(value), `external URL ${value} in ${path}`);
  }
  assert(!/\beval\s*\(|\bnew\s+Function\s*\(/.test(source), `dynamic code in ${path}`);
  if (t3) {
    t3Total += bytes.length;
    t3Compressed += gzipSync(bytes).length;
    continue;
  }
  compressed += gzipSync(bytes).length;
  assert(bytes.length < 512 * 1024, `unexpectedly large individual text asset: ${path}`);
}
assert(t3Unknown.size === 0, `T3 entry: ${t3Unknown.size} external URLs not in src/t3-entry/dist-urls.txt:\n${[...t3Unknown].sort().join('\n')}`);
if (t3Build) console.log(`T3 entry: text ${t3Total}, gzip ${t3Compressed} bytes, reviewed URLs ${t3Urls.size}`);
// Package 1's empty-shell cap covered one screen; this build has chat, first run
// and approval entries. Keep both a whole-window cap and a compressed payload cap.
// 20.11.2026 отображение чата как в T3 (решение основателя 20.11): предел на один файл 256 -> 512 КиБ (кусок разметки react-markdown + remark 388 КБ; katex 259 КБ).
// 19.11 fix QA A: 512 -> 544 KiB for the «Подтверждения» section, live answer text and the approval details.
// 20.11 отображение чата как в T3 (решение основателя 20.11): 576 -> 1216 КиБ (факт 1187946 Б + 5 %: разметка и katex, ленивые куски).
// 20.11 окно-как-T3: 544 -> 576 KiB: значки поставщиков из T3 (OpenAI, Claude, Anthropic, OpenRouter, Ollama) и стили чата/окна защиты; предел gzip не менялся.
// 20.11 меню режима доступа из T3 (@base-ui/react/menu, решение основателя): 1216 -> 1368 КиБ (факт 1334008 Б + 5 %).
assert(total < 1368 * 1024, `window text assets unexpectedly large: ${total} bytes`);
// 20.11.2026, новый вид окна по design/DESIGN.md (боковая колонка, список чатов, токены системы, старая разметка и стили заменены):
// было 163468/163840, стало 163782/171971 (предел = факт + 5 %). Проверка остаётся включённой.
// 20.11.2026, строка поля ввода, ссылки/картинки/шаги в ответах: 165275/173538 (факт + 5 %), было 163782/171971.
// 20.11.2026, ссылки как кнопки, отказ вне папки, ключ: 167329/175695 (факт + 5 %), было 165275/173538.
// 20.11.2026, круг 2 (настоящий путь папки, фраза о ключе при вводе, затемнение узкой колонки): 167615/175996 (факт + 5 %), было 167329/175695.
// 20.11.2026, после установки npm-пакетов T3 (решение CEO): было 175924/175996, предел = факт + 5 % = 184720. Рендер T3 (+180 КБ) в сборку не входит, см. src/t3/render.tsx.
// 20.11.2026, отображение чата как в T3, решение основателя 20.11: было 176070/184720, стало 360071/378075 (предел = факт + 5 %).
// 20.11.2026, меню режима доступа из T3 (@base-ui/react/menu): было 360808/378075, стало 409703/430188 (предел = факт + 5 %).
const GZIP_LIMIT = 430188;
assert(compressed < GZIP_LIMIT, `window compressed text assets unexpectedly large: ${compressed} bytes`);
console.log(`passed: dist has no external network URLs or dynamic code; text ${total}/${1368 * 1024}, gzip ${compressed}/${GZIP_LIMIT} bytes`);
