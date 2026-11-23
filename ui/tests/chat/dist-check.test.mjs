import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const CHECK = fileURLToPath(new URL('../../scripts/check-dist.mjs', import.meta.url));
async function check(files, error, csp) {
  const root = await mkdtemp(join(tmpdir(), 'hoh-dist-gate-'));
  try {
    await mkdir(join(root, 'dist'));
    for (const [name, text] of Object.entries(files)) await writeFile(join(root, 'dist', name), text);
    const env = { ...process.env };
    if (csp !== undefined) {
      env.HOH_TAURI_CONF = join(root, 'tauri.conf.json');
      await writeFile(env.HOH_TAURI_CONF, JSON.stringify({ app: { security: { csp } } }));
    }
    const result = spawnSync(process.execPath, [CHECK], { cwd: root, encoding: 'utf8', timeout: 5000, env });
    if (error) assert(result.status !== 0 && result.stderr.includes(error), result.stderr);
    else assert.equal(result.status, 0, result.stderr);
  } finally { await rm(root, { recursive: true, force: true }); }
}

test('dist gate accepts local assets without network addresses', () => check({ 'index.html': '<script src="./app.js"></script>', 'app.js': 'console.log("hello")' }));
test('dist gate takes only the pages of the window', async () => {
  await check({ 'index.html': '<p></p>', 'legacy.html': '<p></p>', 'approval.html': '<p></p>' });
  for (const page of ['firstrun.html', 'probe.html', 'other.html']) await check({ 'index.html': '<p></p>', [page]: '<p></p>' }, 'страница не окна в dist: ' + page);
});
test('dist gate accepts exact inert namespaces, local events and loopback configuration', () => check({ 'app.js': 'const names=["http://www.w3.org/2000/svg","tauri://resize","http://localhost:8000/v1"]' }));
test('dist gate rejects remote URLs, including diagnostic links', () => check({ 'app.js': 'const x="https://outside.invalid/x"' }, 'external URL'));
test('dist gate rejects real external addresses of every shape', async () => {
  await check({ 'app.js': 'const x="//cdn.example.com/lib.js"' }, 'external URL');
  await check({ 'app.js': 'const x="ws://example.org/socket"' }, 'external URL');
  await check({ 'app.js': 'const x="http://localhost:9999/v1"' }, 'external URL');
});
test('dist gate does not take the tail of a regular expression for an address', () => check({ 'app.js': 'const s=t.replace(/a/g,"b").split(/x//g,r=/y/)' }));
test('dist gate rejects an unrecognized Tauri URI', () => check({ 'app.js': 'const x="tauri://resize-unknown"' }, 'external URL'));
test('dist gate does not whitelist arbitrary loopback addresses', () => check({ 'app.js': 'const x="http://localhost:9999/v1"' }, 'external URL'));
test('dist gate rejects dynamic execution', async () => {
  await check({ 'app.js': 'eval("hello")' }, 'dynamic code');
  await check({ 'app.js': 'new Function("return 1")' }, 'dynamic code');
});
test('dist gate enforces the single-asset cap', () => check({ 'app.js': 'x'.repeat(600000) }, 'individual text asset'));
test('dist gate enforces the aggregate raw cap', () => check({ 'a.js': 'x'.repeat(450000), 'b.js': 'y'.repeat(450000), 'c.js': 'z'.repeat(450000), 'd.js': 'w'.repeat(450000) }, 'window text assets'));
test('dist gate enforces the compressed payload cap', () => check(Object.fromEntries(['a', 'b', 'c', 'd'].map(name => [name + '.js', JSON.stringify(randomBytes(110000).toString('hex'))])), 'compressed text assets'));

// CSP окна (решение CEO 22.11): наружу закрыто всё; ослабленный CSP роняет проверку dist.
const WINDOW_CSP = JSON.parse(readFileSync(new URL('../../../apps/hoh-desktop/tauri.conf.json', import.meta.url), 'utf8')).app.security.csp;
const withDirective = (directive) => WINDOW_CSP.split(';').map((part) => part.trim()).filter((part) => part && !part.startsWith(directive.split(' ')[0] + ' ')).concat(directive).join('; ');
test('dist gate accepts the window CSP as it is', () => check({ 'app.js': 'console.log(1)' }, undefined, WINDOW_CSP));
test('dist gate requires only the narrow WebAssembly execution permission', async () => {
  const message = 'script-src должен разрешать только self и локальную компиляцию WASM';
  await check({ 'app.js': 'console.log(1)' }, message, withDirective("script-src 'self'"));
  await check({ 'app.js': 'console.log(1)' }, message,
    withDirective("script-src 'self' 'wasm-unsafe-eval' 'unsafe-eval'"));
  await check({ 'app.js': 'console.log(1)' }, message,
    withDirective("script-src 'self' 'wasm-unsafe-eval' https://scripts.example.com"));
});
test('dist gate rejects a window CSP that lets anything out', async () => {
  await check({ 'app.js': 'console.log(1)' }, "default-src не 'self'", withDirective('default-src *'));
  for (const directive of ['connect-src ipc: http://ipc.localhost https://api.example.com', 'img-src \'self\' data: blob: https:',
    'font-src \'self\' https://fonts.example.com', 'frame-src https://example.com', 'connect-src ipc: ws:']) {
    await check({ 'app.js': 'console.log(1)' }, 'источник не из списка', withDirective(directive));
  }
  await check({ 'app.js': 'console.log(1)' }, 'директива media-src не из списка', withDirective('media-src https://example.com'));
  await check({ 'app.js': 'console.log(1)' }, 'нет frame-src', WINDOW_CSP.split(';').filter((part) => !part.includes('frame-src')).join(';'));
});
