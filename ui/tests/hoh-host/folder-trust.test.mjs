import { test } from 'node:test';
import assert from 'node:assert/strict';
import { folderTrustView } from '../../src/t3-entry/hoh-screens/folder-trust-view.mjs';

test('folder trust describes the running process and pending change independently', () => {
  const off = folderTrustView({ running: true, applied: false, trusted: false, changed: false });
  assert.match(off.text, /Claude and Codex.*inactive/);
  assert.equal(off.canTrust, true);
  const pending = folderTrustView({ running: true, applied: false, trusted: true, changed: false });
  assert.match(pending.text, /will be enabled from the next message/);
  assert.equal(pending.canTrust, false);
  const changed = folderTrustView({ running: true, applied: true, trusted: false, changed: true });
  assert.match(changed.text, /active.*settings changed/);
  assert.equal(changed.canTrust, true);
  const active = folderTrustView({ running: true, applied: true, trusted: true, changed: false });
  assert.match(active.text, /Claude and Codex.*active/);
  for (const trusted of [false, true]) {
    const stopped = folderTrustView({ running: false, applied: false, trusted, changed: false });
    assert.match(stopped.text, trusted ? /: active$/ : /: inactive$/);
  }
  const isolated = folderTrustView({ applied: false, trusted: false, isolated: true, changed: false });
  assert.equal(isolated.canTrust, false);
});

test('applied folder trust survives host restart and keeps the chat metadata intact', async () => {
  const { readAppliedTrust, writeAppliedTrust } = await import('../../src/t3-entry/hoh-screens/folder-trust-record.mjs');
  const data = new Map([['hoh-host.thread-metadata.v1', JSON.stringify({ chat: { title: 'My chat' } })]]);
  const storage = { getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value) };
  assert.equal(readAppliedTrust(storage, 'chat'), null);
  writeAppliedTrust(storage, 'chat', { cwd: '/work', executor: 'claude' });
  assert.deepEqual(readAppliedTrust(storage, 'chat'), { cwd: '/work', executor: 'claude' });
  assert.equal(JSON.parse(data.get('hoh-host.thread-metadata.v1')).chat.title, 'My chat');
  const afterRestart = folderTrustView({ running: false, applied: true, trusted: true, changed: false });
  assert.match(afterRestart.text, /: active$/);
  writeAppliedTrust(storage, 'chat', null);
  assert.equal(readAppliedTrust(storage, 'chat'), null);
});
