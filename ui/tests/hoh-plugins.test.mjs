import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resolve } from 'node:path';
import { componentHarness, textOf } from './screens-a/component-harness.mjs';

const page = resolve(import.meta.dirname, '../src/t3-entry/hoh-screens/PluginsPage.tsx');

test('opens a compiled plugin from the inventory for the selected session', async () => {
  const calls = [];
  const plugin = {
    id: 'turn-note',
    manifest: { name: 'Последний ход', version: '0.1.0', panel: { title: 'Итог', fields: [{ key: 'turn', label: 'Ход' }] } },
    status: 'mounted', diagnostic: null, data: { turn: 3 },
  };
  const inventory = { async list(id) { calls.push(id); return [plugin]; } };
  const h = componentHarness(page, 'PluginsPage', {}, { inventory, session: 'chat-1' });
  await h.settle();
  assert.deepEqual(calls, ['chat-1']);
  assert.match(h.text(), /Последний ход/);
  assert.match(h.text(), /Active/);
  assert.match(h.text(), /Session panel/);
  h.nodes().find(node => node.props['data-plugin-id'] === 'turn-note').props.onClick();
  await h.settle();
  assert.doesNotMatch(h.text(), /Последний ход/);
  h.unmount();
});

test('recognizes service names and filters the catalog by description', async () => {
  const inventory = { list: async () => [
    { id: 'write-notion', manifest: { name: 'Write to Notion', description: 'Send notes from YouTube to Stripey.' }, status: 'mounted' },
    { id: 'turn-note', manifest: { name: 'Last turn' }, status: 'disabled' },
  ] };
  const h = componentHarness(page, 'PluginsPage', {}, { inventory });
  await h.settle();
  const search = h.nodes().find(node => node.type === 'input');
  search.props.onChange({ target: { value: ' yOuTuBe ' } });
  await h.settle();
  assert.match(h.text(), /Write to Notion/);
  assert.doesNotMatch(h.text(), /Last turn/);
  assert.equal(h.nodes().find(node => node.props.className === 'hoh-plugin-artwork').props.children.props.entity, 'notion');
  const mentions = h.nodes().filter(node => node.props.className === 'hoh-plugin-entity');
  assert.deepEqual(mentions.map(node => node.props.children[0].props.entity), ['youtube', 'rust']);
  h.unmount();
});

test('keeps plugin cards searchable when an older manifest supplies non-string display fields', async () => {
  const inventory = { list: async () => [
    { id: 'fallback-plugin', manifest: { name: 42, description: 17 }, status: 'disabled' },
  ] };
  const h = componentHarness(page, 'PluginsPage', {}, { inventory });
  await h.settle();
  const search = h.nodes().find(node => node.type === 'input');
  search.props.onChange({ target: { value: 'fallback' } });
  await h.settle();
  assert.match(h.text(), /fallback-plugin/);
  assert.match(h.text(), /disabled/);
  search.props.onChange({ target: { value: 'no match' } });
  await h.settle();
  assert.match(h.text(), /No matching plugins/);
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
const creatorMocks = {
  '~/components/chat/ComposerSpeechButton': {
    ComposerSpeechButton: 'voice-start', ComposerSpeechRecordingPill: 'voice-recording',
  },
  './whisper/useComposerSpeechInput': {
    useWhisperDictation: () => ({ available: false, blocksSubmission: false }),
  },
  '../../t3/base/web/components/ui/select': {
    Select: 'select-root', SelectTrigger: 'button', SelectPopup: 'select-popup', SelectItem: 'select-item',
  },
};
const creatorOptions = {
  projects: [{ id: 'checkout', title: 'HoH worktree', path: '/work/hoh' }],
  harnesses: [
    { executor: 'claude', name: 'Claude', model: 'sonnet', available: false, reason: 'Sign in to Claude.' },
    { executor: 'codex', name: 'Codex', model: 'gpt', available: true },
  ],
};

async function filledForm(h) {
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
  const h = componentHarness(creatorPage, 'PluginCreatorPage', creatorMocks, {
    creator, onAddProject: () => added++, onOpenThread: id => opened.push(id),
  });
  await h.settle();
  assert(h.nodes().find(node => node.props['aria-label'] === 'Send plugin request').disabled);
  assert.equal(textOf(h.byId('plugin-creator-project')), 'HoH worktree');
  assert.equal(textOf(h.byId('plugin-creator-harness')), 'Codex');
  h.nodes().find(node => node.props['aria-label'] === 'Add plugin folder').props.onClick();
  assert.equal(added, 1);
  await h.click('Summarize session activity');
  assert.equal(h.byId('plugin-creator-brief').props.value, 'Summarize session activity');
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
    launch: async () => { throw new Error('Choose a folder for the plugin.'); },
  };
  const h = componentHarness(creatorPage, 'PluginCreatorPage', creatorMocks, {
    creator, onAddProject() {}, onOpenThread: () => assert.fail('Opened a failed launch'),
  });
  await h.settle();
  const form = await filledForm(h);
  await form.props.onSubmit({ preventDefault() {} });
  await h.settle();
  assert.match(h.text(), /Choose a folder for the plugin/);
  assert.equal(h.byId('plugin-creator-brief').props.value, '  Add a clock tool.  ');
  assert.equal(h.nodes().find(node => node.props['aria-label'] === 'Send plugin request').disabled, false);
  h.unmount();
});

test('Plugin Creator keeps dictated text in the draft until it is submitted', async () => {
  const launched = [];
  let dictation;
  let blocksSubmission = false;
  const h = componentHarness(creatorPage, 'PluginCreatorPage', {
    ...creatorMocks,
    './whisper/useComposerSpeechInput': {
      useWhisperDictation(options) {
        dictation = options;
        return {
          available: true, configured: true, blocksSubmission, freezesEditor: blocksSubmission,
          state: { phase: blocksSubmission ? 'recording' : 'idle' },
          start() { blocksSubmission = true; },
          stop() {
            blocksSubmission = false;
            options.commitDraft(options.readDraft().text + ' With a voice command.');
          },
        };
      },
    },
  }, {
    creator: {
      options: async () => creatorOptions,
      subscribe: () => () => {},
      launch: async input => { launched.push(input); return 'plugin-chat'; },
    },
    onAddProject() {},
    onOpenThread() {},
  });
  h.window.requestAnimationFrame = () => 0;
  await h.settle();
  await filledForm(h);
  h.nodes().find(node => node.type === 'voice-start').props.onStart();
  h.forceRender();
  await h.nodes().find(node => node.type === 'form').props.onSubmit({ preventDefault() {} });
  assert.equal(launched.length, 0);
  assert.equal(dictation.readDraft().text, '  Add a clock tool.  ');
  h.nodes().find(node => node.type === 'voice-recording').props.onStop();
  await h.settle();
  assert.equal(launched.length, 0);
  assert.match(h.byId('plugin-creator-brief').props.value, /With a voice command/);
  await h.nodes().find(node => node.type === 'form').props.onSubmit({ preventDefault() {} });
  assert.equal(launched.length, 1);
  h.unmount();
});
