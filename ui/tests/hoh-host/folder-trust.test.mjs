import { test } from 'node:test';
import assert from 'node:assert/strict';
import { folderTrustView } from '../../src/t3-entry/hoh-screens/folder-trust-view.mjs';

test('folder trust describes the running process and pending change independently', () => {
  const off = folderTrustView({ running: true, applied: false, trusted: false, changed: false });
  assert.match(off.text, /Claude и Codex.*не применяются/);
  assert.equal(off.canTrust, true);
  const pending = folderTrustView({ running: true, applied: false, trusted: true, changed: false });
  assert.match(pending.text, /не применяются.*следующего хода/);
  assert.equal(pending.canTrust, false);
  const changed = folderTrustView({ running: true, applied: true, trusted: false, changed: true });
  assert.match(changed.text, /применяются.*настройки изменились/);
  assert.equal(changed.canTrust, true);
  const active = folderTrustView({ running: true, applied: true, trusted: true, changed: false });
  assert.match(active.text, /Claude и Codex.*применяются/);
  for (const trusted of [false, true]) {
    const stopped = folderTrustView({ running: false, applied: true, trusted, changed: false });
    assert.match(stopped.text, /^Процесс не запущен; следующий ход/);
    assert.equal(stopped.text.endsWith(trusted ? ' применяются' : ' не применяются'), true);
  }
  const isolated = folderTrustView({ applied: false, trusted: false, isolated: true, changed: false });
  assert.equal(isolated.canTrust, false);
});
