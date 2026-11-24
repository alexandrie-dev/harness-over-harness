import { test } from 'node:test';
import assert from 'node:assert/strict';
import { folderTrustView } from '../../src/t3-entry/hoh-screens/folder-trust-view.mjs';

test('folder trust describes the running process and pending change independently', () => {
  const off = folderTrustView({ applied: false, trusted: false, changed: false });
  assert.equal(off.text, 'Настройки проекта не применяются');
  assert.equal(off.canTrust, true);
  const pending = folderTrustView({ applied: false, trusted: true, changed: false });
  assert.match(pending.text, /не применяются.*следующего хода/);
  assert.equal(pending.canTrust, false);
  const changed = folderTrustView({ applied: true, trusted: false, changed: true });
  assert.match(changed.text, /^Настройки проекта применяются.*настройки изменились/);
  assert.equal(changed.canTrust, true);
  const active = folderTrustView({ applied: true, trusted: true, changed: false });
  assert.equal(active.text, 'Настройки проекта применяются');
  const isolated = folderTrustView({ applied: false, trusted: false, isolated: true, changed: false });
  assert.equal(isolated.canTrust, false);
});
