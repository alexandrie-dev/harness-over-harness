import assert from 'node:assert/strict';
import { readFile, rm } from 'node:fs/promises';
import { after, before, test } from 'node:test';
import { buildHost } from './build.mjs';
import { FakeEngine } from './fake-engine.mjs';

const cwd = '/home/person/Projects/tools';
const marker = 'crates/hoh-plugin-creator/Cargo.toml';
const brief = 'Add a Rust tool that summarizes completed turns.';
const skill = await readFile(new URL('../../src/t3-entry/skills/hoh-plugin-authoring/SKILL.md', import.meta.url), 'utf8');
let built;
let HohHost;
before(async () => {
  built = await buildHost();
  ({ HohHost } = await built.load('host'));
});
after(async () => { if (built) await rm(built.dir, { recursive: true, force: true }); });

async function setup(t, engine = new FakeEngine()) {
  const host = new HohHost({ engine, storage: null, throttleMs: 0 });
  t.after(() => host.stop());
  const api = host.pluginCreator(skill);
  assert.deepEqual((await api.options()).projects, []);
  const changed = [];
  const close = api.subscribe(() => changed.push(true));
  t.after(close);
  await host.handlers().unary['projects.mutate']({
    type: 'project.create', commandId: 'add-checkout', projectId: 'author', title: 'HoH author worktree',
    workspaceRoot: cwd,
  });
  assert(changed.length > 0);
  assert.deepEqual((await api.options()).projects, [{ id: 'author', title: 'HoH author worktree', path: cwd }]);
  return { host, api, engine };
}

for (const executor of ['claude', 'codex']) {
  test(`Plugin Creator launches ${executor} in the chosen checkout with the bundled skill and request`, async t => {
    const { api, engine, host } = await setup(t);
    await host.handlers().unary['server.updateSettings']({ patch: { defaultRuntimeMode: 'approval-required' } });
    engine.files = [{ path: marker, kind: 'file' }];
    const selected = (await api.options()).harnesses.find(item => item.executor === executor);
    assert(selected.available);
    const threadId = await api.launch({ projectId: 'author', executor, brief });
    const prompts = engine.requests.filter(call => call.method === 'session/prompt');
    assert.equal(prompts.length, 1);
    assert.equal(prompts[0].params.sessionId, threadId);
    const [{ text }] = prompts[0].params.contentBlocks;
    assert(text.startsWith('# HoH plugin authoring\n'));
    assert(text.includes('hoh plugin new <id> --template tool --json'));
    assert(text.includes('hoh plugin test <id> --json'));
    assert(text.includes('plugins/turn-note'));
    assert(text.endsWith(brief));
    assert.deepEqual(engine.starts, [{ sessionId: threadId, executor, cwd }]);
    assert.equal(engine.sessionAccess.get(threadId), 'confirm');
  });
}

test('an unrelated folder or unavailable harness never sends a prompt', async t => {
  const engine = new FakeEngine();
  engine.codexDiscovery = { ...engine.codexDiscovery, auth: 'unauthenticated' };
  const { api } = await setup(t, engine);
  const options = await api.options();
  assert.equal(options.harnesses.find(item => item.executor === 'codex').available, false);
  await assert.rejects(api.launch({ projectId: 'author', executor: 'codex', brief }), /not authenticated/);
  await assert.rejects(api.launch({ projectId: 'hoh-project', executor: 'claude', brief }), /active HoH checkout/);
  engine.files = [{ path: 'unrelated/' + marker, kind: 'file' }];
  await assert.rejects(api.launch({ projectId: 'author', executor: 'claude', brief }), /root of a HoH checkout/);
  assert.deepEqual(engine.starts, []);
  assert.equal(engine.requests.some(call => call.method === 'session/prompt'), false);
});
