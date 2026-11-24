import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const ui = resolve(import.meta.dirname, '..');
test('dist chat wording check catches visible labels while preserving protocol identifiers', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'chat-words-'));
  const run = () => spawnSync(process.execPath, [join(ui, 'scripts/check-chat-words.mjs')], {
    cwd: ui, env: { ...process.env, HOH_CHECK_DIST: dir }, encoding: 'utf8',
  });
  try {
    await writeFile(join(dir, 'main-example.js'), 'const p="thread.metadata.update";const x={children:"New chat", "aria-label":"Rename chat"};');
    assert.equal(run().status, 0);
    for (const property of ['children', 'title', 'placeholder', 'aria-label']) {
      await writeFile(join(dir, 'main-example.js'), `const x={${JSON.stringify(property)}:"Rename thread"};`);
      const result = run();
      assert.equal(result.status, 1, property);
      assert.match(result.stderr, /Rename thread/);
    }
    await writeFile(join(dir, 'main-example.js'), 'const x={children:"thread"};');
    assert.equal(run().status, 1);
    await writeFile(join(dir, 'main-example.js'), 'const x=gt("thread");');
    assert.equal(run().status, 1);
    await writeFile(join(dir, 'main-example.js'), 'const x=gt("thread", {context: "label"});');
    assert.equal(run().status, 1);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
