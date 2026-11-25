import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resolve } from 'node:path';
import { componentHarness } from './screens-a/component-harness.mjs';

const page = resolve(import.meta.dirname, '../src/t3-entry/hoh-screens/PluginsPage.tsx');

test('shows a compiled plugin, its status, and the manifest panel for the selected session', async () => {
  const calls = [];
  const inventory = { async list(id) { calls.push(id); return [{ id: 'turn-note', manifest: { name: 'Последний ход', version: '0.1.0', panel: { title: 'Итог', fields: [{ key: 'turn', label: 'Ход' }] } }, status: 'mounted', diagnostic: null, data: { turn: 3 } }]; } };
  const h = componentHarness(page, 'PluginsPage', {}, { inventory, session: 'chat-1' });
  await h.settle();
  assert.deepEqual(calls, ['chat-1']);
  assert.match(h.text(), /Последний ход/);
  assert.match(h.text(), /mounted/);
  assert.match(h.text(), /Итог/);
  assert.match(h.text(), /Ход3/);
  assert(h.nodes().some(node => node.props['data-plugin-id'] === 'turn-note'));
  h.unmount();
});

test('reports inventory failure instead of claiming the plugin list is empty', async () => {
  const h = componentHarness(page, 'PluginsPage', {}, { inventory: { list: async () => { throw new Error('offline'); } } });
  await h.settle();
  assert.match(h.text(), /Could not load plugin diagnostics/);
  assert.doesNotMatch(h.text(), /No plugins installed/);
  h.unmount();
});
