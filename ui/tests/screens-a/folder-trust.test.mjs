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

test('ordinary approval renders English and records a decision', async () => {
  const calls = [];
  const h = componentHarness(resolve(ui, 'src/screens/approval/ApprovalWindow.tsx'), 'ApprovalWindow', {
    '../../lib/humanText': { tidyHuman: text => text },
    '../shared/icons': { Icon: () => null },
    './bridge': { approvalHost: () => ({
      current: async () => ({ approvalId: 'ordinary', risk: 'high', humanText: 'Run a command in /work?', reason: 'This checks the project.', details: 'npm test', expiresAt: 105000 }),
      decide: async (id, outcome) => { calls.push({ id, outcome }); },
    }) },
  });
  await h.settle();
  assert(h.button('Decline'));
  assert(h.button('Allow once'));
  assert.doesNotMatch(h.text(), /[А-Яа-яЁё]/u);
  assert.match(h.text(), /This checks the project/);
  await h.advance(1000);
  await h.click('Allow once');
  assert.deepEqual(calls, [{ id: 'ordinary', outcome: 'allowed-once' }]);
  assert.doesNotMatch(h.text(), /[А-Яа-яЁё]/u);
  h.unmount();
});

test('ordinary approval translates a known host error without losing its meaning', async () => {
  const h = componentHarness(resolve(ui, 'src/screens/approval/ApprovalWindow.tsx'), 'ApprovalWindow', {
    '../../lib/humanText': { tidyHuman: text => text },
    '../shared/icons': { Icon: () => null },
    './bridge': { approvalHost: () => ({ current: async () => { throw 'Подтверждение уже сменилось'; } }) },
  });
  await h.settle();
  assert.match(h.text(), /The confirmation request has changed/);
  assert.doesNotMatch(h.text(), /[А-Яа-яЁё]/u);
  h.unmount();
});
