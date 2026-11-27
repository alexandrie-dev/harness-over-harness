import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { rm } from 'node:fs/promises';
import { buildHost } from './build.mjs';
import { FakeEngine } from './fake-engine.mjs';
import { WireClient } from './wire-client.mjs';

let built;
let createHohLocalSettings;
let connect;
before(async () => {
  built = await buildHost({ 'local-settings': 'src/t3-entry/local-settings.ts' });
  ({ createHohLocalSettings } = await built.load('local-settings'));
  const [{ HohHost }, { WireServer }, { HohWebSocket }] = await Promise.all([
    built.load('host'), built.load('wire'), built.load('socket'),
  ]);
  connect = (storage) => {
    const host = new HohHost({ engine: new FakeEngine(), storage, createLocalSettings: createHohLocalSettings });
    const client = new WireClient(HohWebSocket, new WireServer(host.handlers()));
    return { client, close: () => { client.close(); host.stop(); } };
  };
});
after(async () => {
  if (built) await rm(built.dir, { recursive: true, force: true });
});

function preferences() {
  const values = new Map();
  return {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
  };
}

test('editing a keybinding streams the effective rules and persists them across a restart', async () => {
  const storage = preferences();
  const connected = connect(storage);
  const changedRule = { key: 'cmd+k', command: 'commandPalette.toggle', when: '!terminalFocus' };
  try {
    const { client } = connected;
    const stream = await client.stream('subscribeServerConfig', {});
    await stream.until(event => event.type === 'snapshot');
    const response = await client.call('server.upsertKeybinding', changedRule);
    const rows = response.keybindings.filter(binding => binding.command === changedRule.command);
    assert.equal(rows.length, 1);
    assert.equal(rows[0].shortcut.metaKey, true);
    assert.equal(rows[0].shortcut.modKey, false);
    const updated = await stream.until(event => event.type === 'keybindingsUpdated');
    assert.deepEqual(updated.payload, response);
  } finally {
    connected.close();
  }

  const reopened = connect(storage);
  try {
    const { client } = reopened;
    const config = await client.call('server.getConfig');
    const binding = config.keybindings.find(binding => binding.command === changedRule.command);
    assert.equal(binding.shortcut.metaKey, true);
    const removed = await client.call('server.removeKeybinding', { ...changedRule, key: 'meta+k', when: '! terminalFocus' });
    const restored = removed.keybindings.find(binding => binding.command === changedRule.command);
    assert.equal(restored.shortcut.modKey, true);
  } finally {
    reopened.close();
  }
});

test('unassigning one default shortcut preserves siblings and rejects invalid rules', async () => {
  const connected = connect(preferences());
  try {
    const { client } = connected;
    const disabled = await client.call('server.upsertKeybinding', {
      key: 'mod+n',
      command: 'chat.new',
      when: 'false',
      replace: { key: 'mod+n', command: 'chat.new', when: '!terminalFocus' },
    });
    const bindings = disabled.keybindings.filter(binding => binding.command === 'chat.new');
    assert.equal(bindings.length, 2);
    assert(bindings.some(binding => binding.shortcut.shiftKey && binding.shortcut.key === 'o'));
    assert(bindings.some(binding => binding.shortcut.key === 'n' && binding.whenAst?.name === 'false'));

    const invalid = await client.request('server.upsertKeybinding', {
      key: 'mod+',
      command: 'chat.new',
      when: 'terminalFocus &&',
    });
    assert.equal(invalid._tag, 'Failure');
    assert.equal(invalid.cause[0].error._tag, 'KeybindingsConfigParseError');
    assert.deepEqual((await client.call('server.getConfig')).keybindings, disabled.keybindings);
  } finally {
    connected.close();
  }
});

test('local model and provider preferences survive a restart without changing the harness', async () => {
  const storage = preferences();
  const connected = connect(storage);
  const favorite = { provider: 'codex', model: 'gpt-6-sol', favorite: true };
  try {
    const { client } = connected;
    await client.call('server.updateSettings', {
      patch: { setModelFavorites: [favorite], setModelsHidden: [{ provider: 'claudeAgent', model: 'claude-opus', hidden: true }] },
      providerInstanceMutation: {
        operation: 'upsert', instanceId: 'codex', instance: { driver: 'codex', displayName: 'Codex work' },
      },
    });
    const refreshed = await client.call('server.refreshProviders', { refreshModels: true });
    assert.equal(refreshed.providers.find(provider => provider.instanceId === 'codex').displayName, 'Codex work');
  } finally {
    connected.close();
  }

  const reopened = connect(storage);
  try {
    const { client } = reopened;
    const config = await client.call('server.getConfig');
    assert.equal(config.providers.find(provider => provider.instanceId === 'codex').displayName, 'Codex work');
    assert.deepEqual(config.settings.favorites, [{ provider: 'codex', model: 'gpt-6-sol' }]);
    assert.deepEqual(config.settings.providerModelPreferences.claudeAgent.hiddenModels, ['claude-opus']);
    assert.equal(config.settings.providerInstances.pi.enabled, true);
    const settings = await client.call('server.updateSettings', {
      patch: {},
      providerInstanceMutation: { operation: 'remove', instanceId: 'codex' },
    });
    assert.equal(settings.providerInstances.codex, undefined);
    assert.equal(settings.favorites.length, 1);
  } finally {
    reopened.close();
  }
});

test('Snooze presets and the default permission mode persist in window settings', async () => {
  const storage = preferences();
  const snoozePresets = [{ kind: 'after', minutes: 45 }, { kind: 'at', day: 'tomorrow', time: '08:00' }];
  const defaultRuntimeMode = 'approval-required';
  const connected = connect(storage);
  try {
    const settings = await connected.client.call('server.updateSettings', {
      patch: { snoozePresets, defaultRuntimeMode },
    });
    assert.deepEqual(settings.snoozePresets, snoozePresets);
    assert.equal(settings.defaultRuntimeMode, defaultRuntimeMode);
  } finally {
    connected.close();
  }

  const reopened = connect(storage);
  try {
    const settings = await reopened.client.call('server.getSettings');
    assert.deepEqual(settings.snoozePresets, snoozePresets);
    assert.equal(settings.defaultRuntimeMode, defaultRuntimeMode);
  } finally {
    reopened.close();
  }
});
