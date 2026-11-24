import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resolve } from 'node:path';
import { componentHarness } from './component-harness.mjs';

const ui = resolve(import.meta.dirname, '../..');
for (const [description, humanText, label] of [
  ['grant', 'Trust project settings in /work/project (Claude and Codex)?', 'Trust folder'],
  ['revoke', 'Revoke trust for project settings in /work/project (Claude and Codex)?', 'Revoke trust'],
]) {
  test(`folder trust ${description} has an accurate label and submits the existing decision`, async () => {
    const calls = [];
    const h = componentHarness(resolve(ui, 'src/screens/approval/ApprovalWindow.tsx'), 'ApprovalWindow', {
      '../../lib/hostError.mjs': { errorText: (_error, fallback) => fallback },
      '../../lib/humanText': { tidyHuman: text => text },
      '../shared/icons': { Icon: () => null },
      './bridge': { approvalHost: () => ({
        current: async () => ({ approvalId: 'a', risk: 'high', humanText, expiresAt: 105000, folderTrust: true }),
        decide: async (id, outcome) => { calls.push({ id, outcome }); },
      }) },
    });
    await h.settle();
    assert(h.button(label));
    assert.equal(h.button('Allow once'), undefined);
    await h.advance(1000);
    await h.click(label);
    assert.deepEqual(calls, [{ id: 'a', outcome: 'allowed-once' }]);
    assert(h.text().includes('Confirmed'));
    h.unmount();
  });
}
