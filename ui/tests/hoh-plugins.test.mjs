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

test('shows the empty state only after the engine returns an empty inventory', async () => {
  const h = componentHarness(page, 'PluginsPage', {}, { inventory: { list: async () => [] } });
  await h.settle();
  assert.match(h.text(), /No plugins installed/);
  assert(h.nodes().some(node => node.props['data-plugin-empty'] !== undefined));
  h.unmount();
});

const creatorPage = resolve(import.meta.dirname, '../src/t3-entry/hoh-screens/PluginCreatorPage.tsx');
const creatorOptions = {
  projects: [{ id: 'checkout', title: 'HoH worktree', path: '/work/hoh' }],
  harnesses: [
    { executor: 'claude', name: 'Claude', model: 'sonnet', available: false, reason: 'Sign in to Claude.' },
    { executor: 'codex', name: 'Codex', model: 'gpt', available: true },
  ],
};

async function filledForm(h) {
  h.byId('plugin-creator-project').props.onChange({ target: { value: 'checkout' } });
  h.byId('plugin-creator-harness').props.onChange({ target: { value: 'codex' } });
  h.byId('plugin-creator-brief').props.onChange({ target: { value: '  Add a clock tool.  ' } });
  await h.settle();
  return h.nodes().find(node => node.type === 'form');
}

test('Plugin Creator collects the checkout, harness and brief and opens the launched chat once', async () => {
  const launched = [];
  const opened = [];
  let added = 0;
  const creator = {
    options: async () => creatorOptions,
    subscribe: () => () => {},
    launch: async input => { launched.push(input); return 'plugin-chat'; },
  };
  const h = componentHarness(creatorPage, 'PluginCreatorPage', {}, {
    creator, onAddProject: () => added++, onOpenThread: id => opened.push(id),
  });
  await h.settle();
  assert(h.button('Start authoring').disabled);
  assert.match(h.text(), /Sign in to Claude/);
  await h.click('Add project');
  assert.equal(added, 1);
  const form = await filledForm(h);
  const event = { preventDefault() {} };
  await Promise.all([form.props.onSubmit(event), form.props.onSubmit(event)]);
  await h.settle();
  assert.equal(launched.length, 1);
  assert.equal(launched[0].projectId, 'checkout');
  assert.equal(launched[0].executor, 'codex');
  assert.equal(launched[0].brief, 'Add a clock tool.');
  assert.deepEqual(opened, ['plugin-chat']);
  h.unmount();
});

test('Plugin Creator keeps the request available when starting a harness fails', async () => {
  const creator = {
    options: async () => creatorOptions,
    subscribe: () => () => {},
    launch: async () => { throw new Error('Choose a HoH checkout.'); },
  };
  const h = componentHarness(creatorPage, 'PluginCreatorPage', {}, {
    creator, onAddProject() {}, onOpenThread: () => assert.fail('Opened a failed launch'),
  });
  await h.settle();
  const form = await filledForm(h);
  await form.props.onSubmit({ preventDefault() {} });
  await h.settle();
  assert.match(h.text(), /Choose a HoH checkout/);
  assert.equal(h.byId('plugin-creator-brief').props.value, '  Add a clock tool.  ');
  assert.equal(h.button('Start authoring').disabled, false);
  h.unmount();
});
