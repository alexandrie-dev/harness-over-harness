// HOH-41 package 1 on the HoH host: what T3's composer promises ("@ files, $ skills, / commands") and pictures.
import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { rm } from 'node:fs/promises';
import { buildHost } from './build.mjs';
import { DEFAULT_FOLDER, FakeEngine } from './fake-engine.mjs';
import { WireClient } from './wire-client.mjs';

let built; let mod;
before(async () => {
  built = await buildHost();
  mod = { host: await built.load('host'), wire: await built.load('wire'), socket: await built.load('socket') };
});
after(async () => { await rm(built.dir, { recursive: true, force: true }); });

const SITE = '/home/person/Projects/site';
/** A red 2×2 PNG. */
const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==';
const PNG_BYTES = Buffer.from(PNG, 'base64').length;

function connect(engine) {
  const host = new mod.host.HohHost({ engine, throttleMs: 0, storage: null });
  const client = new WireClient(mod.socket.HohWebSocket, new mod.wire.WireServer(host.handlers()));
  return { host, client };
}
const failureOf = exit => exit.cause[0].error;
const sent = (engine, method) => engine.requests.filter(request => request.method === method);
const upload = (overrides = {}) => ({ type: 'image', id: 'img-1', name: 'red.png', mimeType: 'image/png', sizeBytes: PNG_BYTES, dataUrl: `data:image/png;base64,${PNG}`, ...overrides });
const dispatch = (threadId, messageId, text, attachments = []) => ({
  type: 'message.dispatch', commandId: 'c-' + messageId, threadId, messageId, text, attachments, createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' },
});
async function newThread(client, threadId, projectId = 'hoh-project') {
  await client.call('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'c0-' + threadId, threadId, projectId, title: 'T', createdBy: 'user', creationSource: 'web',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default', branch: null, worktreePath: null,
  });
}
async function siteProject(client) {
  await client.call('projects.mutate', { type: 'project.create', commandId: 'cp', projectId: 'p-site', title: 'site', workspaceRoot: SITE, createWorkspaceRootIfMissing: false, defaultModelSelection: null });
}

test('a picture: T3 gives the bytes, the host keeps them, the message carries them to the engine before the text', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  const persisted = await client.call('assets.persistChatAttachments', { threadId: 'chat-img', messageId: 'm-1', attachments: [upload()] });
  // T3's ChatImageAttachment: no bytes and no path go back to the page.
  assert.deepEqual(persisted, { attachments: [{ type: 'image', id: 'img-1', name: 'red.png', mimeType: 'image/png', sizeBytes: PNG_BYTES }] });
  await newThread(client, 'chat-img');
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-img' });
  await thread.until(value => value.kind === 'snapshot');
  await client.call('orchestration.dispatchCommand', dispatch('chat-img', 'm-1', 'what is on it?', persisted.attachments));
  const prompt = sent(engine, 'session/prompt').at(-1).params;
  assert.deepEqual(prompt.contentBlocks, [
    { type: 'image', data: PNG, mimeType: 'image/png', name: 'red.png' },
    { type: 'text', text: 'what is on it?' },
  ]);
  // The chat shows the picture with the message (T3 hands its local preview over to it), once.
  const shown = await thread.until(value => value.kind === 'snapshot' && value.projection.messages.length === 1 && value.projection.runs[0]?.status === 'running');
  assert.deepEqual(shown.projection.messages[0].attachments, persisted.attachments);
  await new Promise(resolve => setTimeout(resolve, 10));
  const echoed = await client.call('orchestration.getThreadProjection', { threadId: 'chat-img' });
  assert.equal(echoed.messages.filter(message => message.role === 'user').length, 1);
  // A picture alone goes without an empty text block. The bytes are not kept once the engine has them.
  await client.call('assets.persistChatAttachments', { threadId: 'chat-img', messageId: 'm-2', attachments: [upload({ id: 'img-2' })] });
  await client.call('orchestration.dispatchCommand', dispatch('chat-img', 'm-2', '', [{ type: 'image', id: 'img-2', name: 'red.png', mimeType: 'image/png', sizeBytes: PNG_BYTES }]));
  assert.deepEqual(sent(engine, 'session/prompt').at(-1).params.contentBlocks.map(block => block.type), ['image']);
  const again = await client.request('orchestration.dispatchCommand', dispatch('chat-img', 'm-3', 'again', persisted.attachments));
  assert.match(failureOf(again).message, /no longer here/);
});

test('a picture too big, of another type, or not what it says is refused with T3\'s words; nothing reaches the engine', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  const cases = [
    [upload({ sizeBytes: 10 * 1024 * 1024 + 1 }), /too large to attach: a picture can be up to 10 MiB/],
    [upload({ mimeType: 'image/svg+xml', dataUrl: `data:image/svg+xml;base64,${PNG}` }), /not a supported image type\. Attach GIF, JPEG, PNG, or WebP/],
    [upload({ mimeType: 'image/heic', dataUrl: `data:image/heic;base64,${PNG}` }), /not a supported image type/],
    [upload({ dataUrl: `data:image/jpeg;base64,${PNG}` }), /invalid image payload/],
    [upload({ sizeBytes: PNG_BYTES + 1 }), /size does not match its payload/],
    [upload({ dataUrl: 'data:image/png;base64,%%%%' }), /not valid base64/],
    [{ ...upload(), type: 'file' }, /does not support file attachments/],
  ];
  for (const [attachment, message] of cases) {
    const refused = await client.request('assets.persistChatAttachments', { threadId: 't', messageId: 'm', attachments: [attachment] });
    assert.equal(failureOf(refused)._tag, 'PersistChatAttachmentsError');
    assert.match(failureOf(refused).message, message, JSON.stringify(attachment).slice(0, 80));
  }
  const many = Array.from({ length: 101 }, (_, index) => upload({ id: 'img-' + index }));
  assert.match(failureOf(await client.request('assets.persistChatAttachments', { threadId: 't', messageId: 'm', attachments: many })).message, /up to 100 files/);
  // A file T3 staged elsewhere, or a picture never given here, is not sent.
  await newThread(client, 'chat-file');
  const file = await client.request('orchestration.dispatchCommand', dispatch('chat-file', 'm-f', 'read it', [{ type: 'file', id: 'f-1', name: 'a.txt', mimeType: 'text/plain', sizeBytes: 3 }]));
  assert.match(failureOf(file).message, /does not support file attachments/);
  assert.equal(sent(engine, 'session/prompt').length, 0);
});

test('@ lists a folder project\'s files by relative path from the engine; "No project" and a folder the engine refuses give T3\'s empty menu', async () => {
  const engine = new FakeEngine();
  engine.files = [{ path: 'src', kind: 'directory' }, { path: 'src/main.ts', kind: 'file' }, { path: 'logo.png', kind: 'file' }];
  const { client } = connect(engine);
  // No project: nothing is asked of the engine.
  assert.deepEqual(await client.call('projects.searchEntries', { cwd: DEFAULT_FOLDER, query: '', limit: 80 }), { entries: [], truncated: false });
  assert.equal(sent(engine, 'hoh/workspace/entries').length, 0);
  await siteProject(client);
  // A project with no conversation yet: the engine does not know the folder; the menu is empty, not an error.
  assert.deepEqual(await client.call('projects.searchEntries', { cwd: SITE, query: 'src', limit: 80 }), { entries: [], truncated: false });
  await newThread(client, 'chat-site', 'p-site');
  const found = await client.call('projects.searchEntries', { cwd: SITE + '/', query: 'src', limit: 80 });
  assert.deepEqual(found, { entries: [{ path: 'src', kind: 'directory' }, { path: 'src/main.ts', kind: 'file' }], truncated: false });
  assert.deepEqual(sent(engine, 'hoh/workspace/entries').at(-1).params, { cwd: SITE, query: 'src', limit: 80 });
  assert.deepEqual((await client.call('projects.searchEntries', { cwd: SITE, query: '', limit: 80, kind: 'directory' })).entries, [{ path: 'src', kind: 'directory' }]);
  assert.deepEqual((await client.call('projects.searchEntries', { cwd: SITE, query: '', limit: 80, imageOnly: true })).entries, [{ path: 'logo.png', kind: 'file' }]);
  // A folder that is not a project is not passed on.
  const before = sent(engine, 'hoh/workspace/entries').length;
  assert.deepEqual(await client.call('projects.searchEntries', { cwd: '/home/person/.config', query: '', limit: 80 }), { entries: [], truncated: false });
  assert.equal(sent(engine, 'hoh/workspace/entries').length, before);
});

test('/ and $ before the first turn: the folder\'s snapshot is what Claude declared; $skill goes as /skill', async () => {
  const engine = new FakeEngine();
  engine.commands = [{ name: 'review', description: 'Review a pull request', argumentHint: '[pr]' }, { name: 'init' }];
  engine.skills = [{ name: 'release-notes', description: 'Write the release notes' }];
  const { client } = connect(engine);
  const refreshed = await client.call('server.refreshProviders', { instanceId: 'claudeAgent', cwd: DEFAULT_FOLDER, workspaceSnapshotsOnDemand: true });
  const [provider] = refreshed.providers;
  assert.equal(provider.instanceId, 'claudeAgent');
  const [snapshot] = provider.workspaceSnapshots;
  assert.equal(snapshot.cwd, DEFAULT_FOLDER);
  assert.equal(snapshot.slashCommandsPending, undefined);
  assert.deepEqual(snapshot.slashCommands, [{ name: 'review', description: 'Review a pull request', input: { hint: '[pr]' } }, { name: 'init' }]);
  assert.deepEqual(snapshot.skills, [{ name: 'release-notes', path: 'release-notes', enabled: true, description: 'Write the release notes' }]);
  assert.equal(sent(engine, 'session/prompt').length, 0);
  assert.deepEqual(sent(engine, 'hoh/executor/commands').at(-1).params, { executor: 'claude', cwd: DEFAULT_FOLDER });

  await newThread(client, 'chat-skill');
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-skill' });
  await thread.until(value => value.kind === 'snapshot');
  await client.call('orchestration.dispatchCommand', dispatch('chat-skill', 'm-s', '$release-notes for v2, not $5k or $unknown'));
  assert.deepEqual(sent(engine, 'session/prompt').at(-1).params.contentBlocks, [{ type: 'text', text: '/release-notes for v2, not $5k or $unknown' }]);
  await new Promise(resolve => setTimeout(resolve, 10));
  const projection = await client.call('orchestration.getThreadProjection', { threadId: 'chat-skill' });
  // The person's words stay as typed, and the engine's echo is the same message.
  assert.deepEqual(projection.messages.filter(message => message.role === 'user').map(message => message.text), ['$release-notes for v2, not $5k or $unknown']);

  // Claude could not be asked: T3's pending snapshot, which it asks for again later.
  engine.commandsFail = true;
  const pending = await client.call('server.refreshProviders', { instanceId: 'claudeAgent', cwd: DEFAULT_FOLDER });
  assert.equal(pending.providers[0].workspaceSnapshots[0].slashCommandsPending, true);
  assert.deepEqual(pending.providers[0].workspaceSnapshots[0].slashCommands, []);
  // Without a folder the answer is the machine's provider, as before.
  assert.equal((await client.call('server.refreshProviders', {})).providers[0].workspaceSnapshots, undefined);
});
