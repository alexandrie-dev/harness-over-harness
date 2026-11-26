import assert from 'node:assert/strict';
import { readFile, rm } from 'node:fs/promises';
import { after, before, test } from 'node:test';
import { buildHost } from './build.mjs';
import { FakeEngine } from './fake-engine.mjs';

const cwd = '/home/person/Projects/tools';
const brief = 'Write a Markdown skill for reviewing changes.';
const skill = await readFile(new URL('../../src/t3-entry/skills/hoh-plugin-authoring/SKILL.md', import.meta.url), 'utf8');
const guide = await readFile(new URL('../../../docs/plugin-authoring-kit.md', import.meta.url), 'utf8');
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
  const api = host.pluginCreator(skill, guide);
  assert.deepEqual((await api.options()).projects, []);
  const changed = [];
  const close = api.subscribe(() => changed.push(true));
  t.after(close);
  await host.handlers().unary['projects.mutate']({
    type: 'project.create', commandId: 'add-plugin-folder', projectId: 'author', title: 'Standalone plugin project',
    workspaceRoot: cwd,
  });
  assert(changed.length > 0);
  assert.deepEqual((await api.options()).projects, [{ id: 'author', title: 'Standalone plugin project', path: cwd }]);
  return { host, api, engine };
}

for (const executor of ['claude', 'codex']) {
  test(`Plugin Creator launches ${executor} in an ordinary folder with the bundled standalone workflow`, async t => {
    const { api, engine, host } = await setup(t);
    await host.handlers().unary['server.updateSettings']({ patch: { defaultRuntimeMode: 'approval-required' } });
    const selected = (await api.options()).harnesses.find(item => item.executor === executor);
    assert(selected.available);
    const threadId = await api.launch({ projectId: 'author', executor, brief });
    const prompts = engine.requests.filter(call => call.method === 'session/prompt');
    assert.equal(prompts.length, 1);
    assert.equal(prompts[0].params.sessionId, threadId);
    const [{ text }] = prompts[0].params.contentBlocks;
    assert(text.startsWith('# Author an external HoH plugin\n'));
    assert.match(text, /Do not require Git, a HoH checkout, or a build of the app/);
    assert.match(text, /ask exactly one design question per turn/);
    assert.match(text, /Wait for approval before scaffolding/);
    assert.match(text, /SKILL\.md and hoh-package\.json/);
    assert.doesNotMatch(text, /cargo run --offline -p hoh/);
    assert(!engine.requests.some(call => call.method === 'hoh/workspace/entries'));
    assert(text.endsWith(brief));
    const user = host.threads.get(threadId).projection().messages.find(message => message.role === 'user');
    assert.equal(user.text, brief);
    const card = host.home.projectEntries().find(entry => entry.id === threadId);
    assert.equal(card.pluginCreator, true);
    assert.equal(card.projectId, 'author');
    assert.deepEqual(engine.starts, [{ sessionId: threadId, executor, cwd }]);
    assert.equal(engine.sessionAccess.get(threadId), 'confirm');
  });
}

test('replays old Plugin Creator chats with the brief visible and leaves later messages intact', async () => {
  const { ThreadModel } = await built.load('threads');
  const { pluginAuthoringPrefixes } = await built.load('plugin-creator');
  const previousInstructions = skill.slice(skill.indexOf('Work in the selected HoH checkout.')).trim();
  const original = `# HoH plugin authoring\n\n${previousInstructions}`;
  const previous = skill.slice(skill.indexOf('# HoH plugin authoring')).trim();
  for (const instructions of [original, previous]) {
    const thread = new ThreadModel({ id: 'legacy', title: 'Renamed plugin chat', createdAt: 1, model: 'model', mode: 'confirm' });
    thread.pluginAuthoringPrefixes = pluginAuthoringPrefixes(skill, guide);
    const savedPrompt = instructions + `\n\n## Plugin request\n\n${brief}`;
    for (const seq of [1, 2]) {
      thread.applyEvent({
        seq, time: seq, type: 'user/message', data: {
          id: String(seq), source: { kind: 'user' }, content: [{ type: 'text', text: savedPrompt }],
        },
      });
    }
    const messages = thread.projection().messages.filter(message => message.role === 'user');
    assert.deepEqual(messages.map(message => message.text), [brief, savedPrompt]);
    assert.equal(thread.home().pluginCreator, true);
  }
});

test('a missing folder or unavailable harness never sends a prompt', async t => {
  const engine = new FakeEngine();
  engine.codexDiscovery = { ...engine.codexDiscovery, auth: 'unauthenticated' };
  const { api } = await setup(t, engine);
  const options = await api.options();
  assert.equal(options.harnesses.find(item => item.executor === 'codex').available, false);
  await assert.rejects(api.launch({ projectId: 'author', executor: 'codex', brief }), /not authenticated/);
  await assert.rejects(api.launch({ projectId: 'hoh-project', executor: 'claude', brief }), /Choose a folder for the plugin/);
  assert.deepEqual(engine.starts, []);
  assert.equal(engine.requests.some(call => call.method === 'session/prompt'), false);
});
