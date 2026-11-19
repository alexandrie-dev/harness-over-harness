import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const CHECK = fileURLToPath(new URL('../../scripts/check-dist.mjs', import.meta.url));
async function check(files, error) {
  const root = await mkdtemp(join(tmpdir(), 'hoh-dist-gate-'));
  try {
    await mkdir(join(root, 'dist'));
    for (const [name, text] of Object.entries(files)) await writeFile(join(root, 'dist', name), text);
    const result = spawnSync(process.execPath, [CHECK], { cwd: root, encoding: 'utf8', timeout: 5000 });
    if (error) assert(result.status !== 0 && result.stderr.includes(error), result.stderr);
    else assert.equal(result.status, 0, result.stderr);
  } finally { await rm(root, { recursive: true, force: true }); }
}

test('dist gate accepts local assets without network addresses', () => check({ 'index.html': '<script src="./app.js"></script>', 'app.js': 'console.log("hello")' }));
test('dist gate accepts exact inert namespaces, local events and loopback configuration', () => check({ 'app.js': 'const names=["http://www.w3.org/2000/svg","tauri://resize","http://localhost:8000/v1"]' }));
test('dist gate rejects remote URLs, including diagnostic links', () => check({ 'app.js': 'const x="https://outside.invalid/x"' }, 'external URL'));
test('dist gate rejects an unrecognized Tauri URI', () => check({ 'app.js': 'const x="tauri://resize-unknown"' }, 'external URL'));
test('dist gate does not whitelist arbitrary loopback addresses', () => check({ 'app.js': 'const x="http://localhost:9999/v1"' }, 'external URL'));
test('dist gate rejects dynamic execution', async () => {
  await check({ 'app.js': 'eval("hello")' }, 'dynamic code');
  await check({ 'app.js': 'new Function("return 1")' }, 'dynamic code');
});
test('dist gate enforces the single-asset cap', () => check({ 'app.js': 'x'.repeat(300000) }, 'individual text asset'));
test('dist gate enforces the aggregate raw cap', () => check({ 'a.js': 'x'.repeat(200000), 'b.js': 'y'.repeat(200000), 'c.js': 'z'.repeat(200000) }, 'window text assets'));
test('dist gate enforces the compressed payload cap', () => check({ 'a.js': JSON.stringify(randomBytes(110000).toString('hex')), 'b.js': JSON.stringify(randomBytes(110000).toString('hex')) }, 'compressed text assets'));
