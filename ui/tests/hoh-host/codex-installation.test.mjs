import assert from 'node:assert/strict';
import { rm } from 'node:fs/promises';
import { test } from 'node:test';
import { buildHost } from './build.mjs';

test('Codex provider displays the engine diagnostic for an incomplete installation', async t => {
  const built = await buildHost();
  t.after(() => rm(built.dir, { recursive: true, force: true }));
  const { codexProvider } = await built.load('config');
  const detail = 'The Codex CLI installation is missing an executable codex-code-mode-host.';
  const discovery = { installed: false, version: null, versionStatus: 'unknown', auth: 'unknown', models: [] };

  const provider = codexProvider({ ...discovery, detail }, new Date().toISOString());
  assert.equal(provider.status, 'error');
  assert.equal(provider.message, detail);
  assert.match(codexProvider(discovery, new Date().toISOString()).message, /was not found on PATH/);
});

test('Codex health uses the model catalog even when the CLI version is unavailable', async t => {
  const built = await buildHost();
  t.after(() => rm(built.dir, { recursive: true, force: true }));
  const { codexProvider } = await built.load('config');
  const discovery = {
    installed: true,
    version: null,
    versionStatus: 'unknown',
    auth: 'authenticated',
    models: [{ id: 'gpt-6-sol' }],
  };
  const provider = codexProvider(discovery, new Date().toISOString());
  assert.equal(provider.status, 'ready');
  assert.equal(provider.models[0].slug, 'gpt-6-sol');
});
