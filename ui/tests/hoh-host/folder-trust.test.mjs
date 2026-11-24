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
    assert.match(stopped.text, trusted ? /enabled from the next message/ : /^No process running;.*will be inactive/);
  }
  const isolated = folderTrustView({ applied: false, trusted: false, isolated: true, changed: false });
  assert.equal(isolated.canTrust, false);
});
