import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resolve } from 'node:path';
import { componentHarness } from './component-harness.mjs';

test('rebind confirmation preserves the exact signed folder text', async () => {
  const text = 'The folder for this chat has changed: «/work/исполнитель«»‹›». Rebind this chat to this folder?';
  const h = componentHarness(resolve(ui, 'src/screens/approval/ApprovalWindow.tsx'), 'ApprovalWindow', {
    '../../lib/hostError.mjs': { errorText: (_error, fallback) => fallback },
    '../../lib/humanText': { tidyHuman: value => value.replace(/\s*«»/g, '') },
    '../shared/icons': { Icon: () => null },
    './bridge': { approvalHost: () => ({ current: async () => ({ approvalId: 'native-preflight-a', risk: 'high', humanText: text, expiresAt: 105000 }) }) },
  });
  await h.settle();
  assert(h.text().includes(text));
  h.unmount();
});

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


test('FolderTrust does not send a tilde fallback after cwd lookup fails and retries later', async () => {
  const calls = []; let available = false;
  const h = componentHarness(resolve(ui, 'src/t3-entry/hoh-screens/FolderTrust.tsx'), 'FolderTrust', {}, { threadId: 'saved', fallbackCwd: '~/Documents/HoH', isServerThread: true });
  h.window.__HOH_TRUST_ENGINE__ = { request: async (method, params) => {
    calls.push({ method, params });
    if (method === 'hoh/executor/access/get') return { session: { executor: 'claude' }, effective: { flags: [] } };
    if (method === 'session/working-directory/get') { if (!available) throw Error('unavailable'); return { cwd: '/work/project' }; }
    if (method === 'hoh/executor/trust/get') return { cwd: params.cwd, executor: 'claude', trusted: false, changed: false };
    throw Error(method);
  } };
  await h.event('focus');
  assert.equal(calls.some(call => call.method === 'hoh/executor/trust/get'), false);
  assert.match(h.text(), /unknown/i);
  available = true; await h.event('focus');
  assert.deepEqual(calls.filter(call => call.method === 'hoh/executor/trust/get').map(call => call.params.cwd), ['/work/project']);
  h.unmount();
});

for (const executor of ['claude', 'codex']) {
  test(`FolderTrust asks the host to revoke ${executor} project trust`, async () => {
    const calls = [];
    const h = componentHarness(resolve(ui, 'src/t3-entry/hoh-screens/FolderTrust.tsx'), 'FolderTrust', {}, { threadId: 'saved', fallbackCwd: '/work/project', isServerThread: true });
    h.window.__HOH_TRUST_ENGINE__ = { request: async (method, params) => {
      calls.push({ method, params });
      if (method === 'session/working-directory/status') return { changed: false };
      if (method === 'hoh/executor/access/get') return { session: { executor }, effective: { flags: [] } };
      if (method === 'session/working-directory/get') return { cwd: '/work/project' };
      if (method === 'hoh/executor/trust/get') return { cwd: params.cwd, executor, trusted: true, changed: false };
      if (method === 'hoh/executor/trust/set') return {};
      throw Error(method);
    } };
    await h.event('focus');
    await h.settle();
    await h.click('Revoke trust…');
    assert.deepEqual({ ...calls.find(call => call.method === 'hoh/executor/trust/set')?.params }, { executor, cwd: '/work/project', trust: false });
    h.unmount();
  });
}

test('a busy conversation explains why trust cannot yet be revoked', async () => {
  const h = componentHarness(resolve(ui, 'src/t3-entry/hoh-screens/FolderTrust.tsx'), 'FolderTrust', {}, { threadId: 'saved', fallbackCwd: '/work/project', isServerThread: true });
  h.window.__HOH_TRUST_ENGINE__ = { request: async (method, params) => {
    if (method === 'session/working-directory/status') return { changed: false };
    if (method === 'hoh/executor/access/get') return { session: { executor: 'claude' }, effective: { flags: [] } };
    if (method === 'session/working-directory/get') return { cwd: '/work/project' };
    if (method === 'hoh/executor/trust/get') return { cwd: params.cwd, executor: 'claude', trusted: true, changed: false };
    if (method === 'hoh/executor/trust/set') throw { code: 'trust-revoke-busy' };
    throw Error(method);
  } };
  await h.event('focus'); await h.settle();
  await h.click('Revoke trust…');
  assert.match(h.text(), /wait for.*finish/i);
  h.unmount();
});

test('changed chat folder requires an explicit rebind of the inspected identity', async () => {
  const calls = []; let changed = true; let reject = true;
  const identity = { device: '123', inode: '456' };
  const h = componentHarness(resolve(ui, 'src/t3-entry/hoh-screens/FolderTrust.tsx'), 'FolderTrust', {}, { threadId: 'saved', fallbackCwd: '/work/project', isServerThread: true });
  h.window.__HOH_TRUST_ENGINE__ = { request: async (method, params) => {
    calls.push({ method, params });
    if (method === 'session/working-directory/status') return { cwd: '/work/project', rootIdentity: identity, changed };
    if (method === 'session/working-directory/rebind') { if (reject) throw Error('declined'); changed = false; return {}; }
    if (method === 'hoh/executor/access/get') return { session: { executor: 'claude' }, effective: { flags: [] } };
    if (method === 'session/working-directory/get') return { cwd: '/work/project' };
    if (method === 'hoh/executor/trust/get') return { cwd: params.cwd, executor: 'claude', trusted: false, changed: false };
    throw Error(method);
  } };
  await h.event('focus');
  assert.match(h.text(), /folder for this chat has changed/i);
  assert.equal(calls.some(call => call.method === 'session/working-directory/rebind'), false);
  await h.click('Rebind…');
  assert.match(h.text(), /not confirmed/i);
  assert.match(h.text(), /folder for this chat has changed/i);
  reject = false;
  await h.click('Rebind…');
  const request = calls.filter(call => call.method === 'session/working-directory/rebind').at(-1).params;
  assert.equal(request.sessionId, 'saved');
  assert.equal(request.path, '/work/project');
  assert.equal(request.rootIdentity.device, '123');
  assert.equal(request.rootIdentity.inode, '456');
  assert.doesNotMatch(h.text(), /folder for this chat has changed/i);
  h.unmount();
});
