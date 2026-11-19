// В компонентах экранов (кроме tokens.css) не должно быть литеральных цветов и размеров:
// всё берётся через var(--…). Условия медиазапросов — исключение: var() там не работает.
import { readdir, readFile } from 'node:fs/promises';
import { join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(import.meta.dirname, '../..');
const dirs = ['src/screens/firstrun', 'src/screens/approval', 'src/screens/shared'];
const rules = [
  ['цвет #…', /#[0-9a-fA-F]{3,8}\b/],
  ['rgb()/rgba()/hsl()', /\b(?:rgba?|hsla?)\s*\(/],
  ['oklch()/oklab()/color-mix()', /\b(?:oklch|oklab|color-mix)\s*\(/],
  ['размер в px/rem/em/dvh/svh/lvh/vh/vw/pt', /(?<![\w-])-?\d*\.?\d+(?:px|rem|em|[dsl]?v[hw]|pt)\b/i],
  ['время в ms/s', /(?<![\w#.-])\d*\.?\d+m?s\b/],
];
async function files(dir) {
  const out = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...await files(path));
    else if (/\.(css|tsx|ts|html)$/.test(entry.name)) out.push(path);
  }
  return out;
}
export function findLiterals(source) {
  const found = [];
  source.split('\n').forEach((line, i) => {
    if (/^\s*@media\b/.test(line) || /^\s*(\/\/|\/\*|\*)/.test(line)) return;
    for (const [name, re] of rules) {
      const m = line.match(re);
      if (m) found.push({ name, line: i + 1, value: m[0] });
    }
  });
  return found;
}
export async function checkScreens() {
  let scanned = 0; const found = [];
  for (const dir of dirs) for (const path of await files(join(root, dir))) {
    scanned++;
    for (const item of findLiterals(await readFile(path, 'utf8'))) found.push({ path: relative(root, path), ...item });
  }
  return { scanned, found };
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const { scanned, found } = await checkScreens();
  for (const item of found) console.log(`${item.path}:${item.line}: ${item.name}: ${item.value}`);
  console.log(found.length ? `FAIL: ${found.length} литералов в ${scanned} файлах` : `passed: ни одного литерального цвета или размера в ${scanned} файлах экранов`);
  process.exitCode = found.length ? 1 : 0;
}
