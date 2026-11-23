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

test('an attached image context link stays in the conversation but is not duplicated in Claude text', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  await client.call('assets.persistChatAttachments', { threadId: 'chat-ref', messageId: 'm-ref', attachments: [upload()] });
  await newThread(client, 'chat-ref');
  const text = 'Describe this\n![red.png](t3-context://v1/image/image_img-1)';
  await client.call('orchestration.dispatchCommand', dispatch('chat-ref', 'm-ref', text, [{ type: 'image', id: 'img-1', name: 'red.png', mimeType: 'image/png', sizeBytes: PNG_BYTES }]));
  assert.deepEqual(sent(engine, 'session/prompt').at(-1).params.contentBlocks, [
    { type: 'image', data: PNG, mimeType: 'image/png', name: 'red.png' },
    { type: 'text', text: 'Describe this' },
  ]);
  const projection = await client.call('orchestration.getThreadProjection', { threadId: 'chat-ref' });
  assert.equal(projection.messages.find(message => message.role === 'user').text, text);
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
  // A folder the engine does not know (a project added before it learned project folders): the menu is empty, not an error.
  engine.folders.add('/home/person/Projects/old');
  await client.call('projects.mutate', { type: 'project.create', commandId: 'co', projectId: 'p-old', title: 'old', workspaceRoot: '/home/person/Projects/old', createWorkspaceRootIfMissing: false, defaultModelSelection: null });
  engine.projectFolders.delete('/home/person/Projects/old');
  assert.deepEqual(await client.call('projects.searchEntries', { cwd: '/home/person/Projects/old', query: 'src', limit: 80 }), { entries: [], truncated: false });
  // "Add project" tells the engine the folder: `@` answers before the project's first chat.
  await siteProject(client);
  assert.deepEqual(sent(engine, 'hoh/workspace/add').at(-1).params, { cwd: SITE });
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
  assert.deepEqual(snapshot.skills, [{ name: 'release-notes', path: 'hoh-skill:release-notes', enabled: true, description: 'Write the release notes' }]);
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

test('/ and $ in a fresh window: "No project" asks for the engine\'s own folder and learns it; a new project is asked by its folder', async () => {
  const engine = new FakeEngine();
  engine.commands = [{ name: 'review' }];
  engine.skills = [{ name: 'release-notes' }];
  const { client } = connect(engine);
  const config = await client.call('server.getConfig', {});
  // Before any chat the host does not know the engine's folder: T3's "No project" shows a placeholder.
  assert.equal(config.scratchWorkspaceRoot, '~/Documents/HoH');
  const first = (await client.call('server.refreshProviders', { instanceId: 'claudeAgent', cwd: '~/Documents/HoH', workspaceSnapshotsOnDemand: true })).providers[0].workspaceSnapshots[0];
  assert.deepEqual([first.cwd, first.slashCommands.map(command => command.name), first.skills.map(skill => skill.name)], ['~/Documents/HoH', ['review'], ['release-notes']]);
  assert.deepEqual(sent(engine, 'hoh/executor/commands').at(-1).params, { executor: 'claude' });
  assert.equal((await client.call('server.getConfig', {})).scratchWorkspaceRoot, DEFAULT_FOLDER);
  // A $skill in "No project" goes as /skill.
  await newThread(client, 'chat-plain');
  await client.call('orchestration.dispatchCommand', dispatch('chat-plain', 'm-p', '$release-notes now'));
  assert.deepEqual(sent(engine, 'session/prompt').at(-1).params.contentBlocks, [{ type: 'text', text: '/release-notes now' }]);
  // A project just added: its folder, before its first chat.
  await siteProject(client);
  const site = (await client.call('server.refreshProviders', { instanceId: 'claudeAgent', cwd: SITE, workspaceSnapshotsOnDemand: true })).providers[0].workspaceSnapshots[0];
  assert.equal(site.slashCommandsPending, undefined);
  assert.deepEqual(sent(engine, 'hoh/executor/commands').at(-1).params, { executor: 'claude', cwd: SITE });
});

test('/ and $ in "No project" of the desktop (the engine runs in its own data): pending until a chat names the folder, then asked by it', async () => {
  const engine = new FakeEngine();
  engine.defaultIsData = true;
  engine.commands = [{ name: 'review' }];
  const { client } = connect(engine);
  const ask = cwd => client.call('server.refreshProviders', { instanceId: 'claudeAgent', cwd, workspaceSnapshotsOnDemand: true }).then(answer => answer.providers[0].workspaceSnapshots[0]);
  const before = await ask('~/Documents/HoH');
  assert.deepEqual([before.slashCommandsPending, before.slashCommands], [true, []]);
  assert.equal((await client.call('server.getConfig', {})).scratchWorkspaceRoot, '~/Documents/HoH');
  await newThread(client, 'chat-plain');
  await client.call('orchestration.dispatchCommand', dispatch('chat-plain', 'm-p', 'hello'));
  assert.equal((await client.call('server.getConfig', {})).scratchWorkspaceRoot, DEFAULT_FOLDER);
  const after = await ask(DEFAULT_FOLDER);
  assert.deepEqual(after.slashCommands.map(command => command.name), ['review']);
  assert.deepEqual(sent(engine, 'hoh/executor/commands').at(-1).params, { executor: 'claude', cwd: DEFAULT_FOLDER });
});

test('Add project: the engine is told the folder and names it as it resolved it; its refusal fails the add in English', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  const project = await client.call('projects.mutate', { type: 'project.create', commandId: 'ch', projectId: 'p-docs', title: 'Docs', workspaceRoot: '~/Documents', createWorkspaceRootIfMissing: false, defaultModelSelection: null });
  assert.equal(project.workspaceRoot, '/home/person/Documents');
  assert.deepEqual(sent(engine, 'hoh/workspace/add').at(-1).params, { cwd: '~/Documents' });
  // The same folder typed another way is already a project.
  const twice = await client.request('projects.mutate', { type: 'project.create', commandId: 'c2', projectId: 'p-docs-2', title: 'Docs', workspaceRoot: '/home/person/Documents', createWorkspaceRootIfMissing: false, defaultModelSelection: null });
  assert.match(failureOf(twice).message, /already a project/);
  engine.refuseFolder = true;
  const refused = await client.request('projects.mutate', { type: 'project.create', commandId: 'c3', projectId: 'p-site', title: 'site', workspaceRoot: SITE, createWorkspaceRootIfMissing: false, defaultModelSelection: null });
  assert.equal(failureOf(refused)._tag, 'ProjectMutationError');
  assert.match(failureOf(refused).message, /cannot be a project: You declined this in the HoH confirmation window\./);
  const shell = await client.stream('orchestration.subscribeShell', {});
  assert.deepEqual((await shell.until(value => value.kind === 'snapshot')).snapshot.projects.map(p => p.id), ['p-docs']);
});

test('a picture T3 uploads: an address for it, its bytes there, the message carries them; another type, size or address is refused', async () => {
  const engine = new FakeEngine();
  const { host, client } = connect(engine);
  const png = Buffer.from(PNG, 'base64');
  const announce = (fields = {}) => client.call('attachments.createUploadUrl', { name: 'red.png', mimeType: 'image/png', sizeBytes: PNG_BYTES, ...fields });
  const minted = await announce();
  assert.match(minted.relativeUrl, /^\/api\/attachments\/upload\/img-[\w.-]+$/);
  assert.equal(minted.relativeUrl, `/api/attachments/upload/${minted.attachmentId}`);
  // The bytes of another type or size, or to an address never given or already used: refused.
  assert.equal((await host.receiveUpload('/api/attachments/upload/img-never', 'image/png', new Blob([png]))).status, 404);
  const wrongType = await announce();
  assert.equal((await host.receiveUpload(wrongType.relativeUrl, 'image/jpeg', new Blob([png]))).status, 415);
  const wrongSize = await announce();
  assert.equal((await host.receiveUpload(wrongSize.relativeUrl, 'image/png', new Blob([png.subarray(1)]))).status, 400);
  assert.equal((await host.receiveUpload(minted.relativeUrl, 'image/png', new Blob([png]))).status, 204);
  assert.equal((await host.receiveUpload(minted.relativeUrl, 'image/png', new Blob([png]))).status, 404);
  // T3 checks an uploaded picture before sending: held here, or T3's "not found" (it uploads again).
  const verified = await client.call('assets.createUrl', { resource: { _tag: 'attachment', attachmentId: minted.attachmentId } });
  assert.match(verified.relativeUrl, /^blob:/);
  assert.equal(Buffer.from(await (await fetch(verified.relativeUrl)).arrayBuffer()).toString('base64'), PNG);
  const lost = await client.request('assets.createUrl', { resource: { _tag: 'attachment', attachmentId: wrongType.attachmentId } });
  assert.equal(failureOf(lost)._tag, 'AssetAttachmentNotFoundError');
  // T3's server refusals for what cannot be a picture for Claude.
  for (const [fields, words] of [
    [{ type: 'file', name: 'report.pdf', mimeType: 'application/pdf' }, /does not support file attachments/],
    [{ name: 'logo.svg', mimeType: 'image/svg+xml' }, /'logo.svg' is not a supported image type/],
    [{ name: 'huge.png', sizeBytes: 10 * 1024 * 1024 + 1 }, /'huge.png' is too large to attach/],
  ]) {
    const refused = await client.request('attachments.createUploadUrl', { name: 'red.png', mimeType: 'image/png', sizeBytes: PNG_BYTES, ...fields });
    assert.equal(failureOf(refused)._tag, 'EnvironmentAuthorizationError');
    assert.match(failureOf(refused).message, words);
  }
  // A removed picture is gone.
  const removed = await announce();
  await client.call('attachments.delete', { attachmentId: removed.attachmentId });
  assert.equal((await host.receiveUpload(removed.relativeUrl, 'image/png', new Blob([png]))).status, 404);
  // The message names the upload; the engine gets the picture's bytes before the text.
  await newThread(client, 'chat-up');
  await client.call('orchestration.dispatchCommand', dispatch('chat-up', 'm-up', 'What is this?', [{ type: 'image', id: minted.attachmentId, name: 'red.png', mimeType: 'image/png', sizeBytes: PNG_BYTES }]));
  assert.deepEqual(sent(engine, 'session/prompt').at(-1).params.contentBlocks, [
    { type: 'image', data: PNG, mimeType: 'image/png', name: 'red.png' },
    { type: 'text', text: 'What is this?' },
  ]);
  // T3 normally deletes the completed upload slot after send. The sent message, not that slot,
  // owns the preview; a later/reminted asset lookup must still return readable bytes.
  await client.call('attachments.delete', { attachmentId: minted.attachmentId });
  const reminted = await client.call('assets.createUrl', { resource: { _tag: 'attachment', attachmentId: minted.attachmentId } });
  assert.equal(reminted.relativeUrl, verified.relativeUrl);
  assert.equal(Buffer.from(await (await fetch(reminted.relativeUrl)).arrayBuffer()).toString('base64'), PNG);
  host.stop();
  await assert.rejects(fetch(reminted.relativeUrl));
});

test('a local reply image is minted only for its conversation and native failures keep a visible reason', async () => {
  const engine = new FakeEngine();
  const reads = [];
  engine.readImageAsset = async input => {
    reads.push(input);
    if (input.path.endsWith('missing.png')) throw new Error('Image unavailable: the file was not found.');
    return {
      dataBase64: PNG,
      mimeType: 'image/png',
      sizeBytes: PNG_BYTES,
      sourcePath: `${DEFAULT_FOLDER}/shots/red.png`,
      fileRevision: 'dev:ino:size:time',
    };
  };
  const { host, client } = connect(engine);
  await newThread(client, 'chat-local');
  const resource = { _tag: 'media-file', threadId: 'chat-local', path: 'shots/red.png', hostOnly: true };
  const first = await client.call('assets.createUrl', { resource });
  assert.match(first.relativeUrl, /^blob:/);
  assert.equal(first.sourcePath, `${DEFAULT_FOLDER}/shots/red.png`);
  assert.equal(first.fileRevision, 'dev:ino:size:time');
  assert.equal(Buffer.from(await (await fetch(first.relativeUrl)).arrayBuffer()).toString('base64'), PNG);
  // A remint revalidates through native code but preserves the live URL while the file revision is unchanged.
  const second = await client.call('assets.createUrl', { resource });
  assert.equal(second.relativeUrl, first.relativeUrl);
  const alias = await client.call('assets.createUrl', { resource: { ...resource, path: 'shots/alias.png' } });
  assert.equal(alias.relativeUrl, first.relativeUrl, 'canonical source path deduplicates aliases');
  assert.deepEqual(reads, [
    { sessionId: 'chat-local', path: 'shots/red.png' },
    { sessionId: 'chat-local', path: 'shots/red.png' },
    { sessionId: 'chat-local', path: 'shots/alias.png' },
  ]);

  const other = await client.request('assets.createUrl', { resource: { ...resource, threadId: 'another-project' } });
  assert.equal(failureOf(other)._tag, 'AssetWorkspaceContextNotFoundError');
  assert.equal(reads.length, 3);
  const missing = await client.request('assets.createUrl', { resource: { ...resource, path: 'missing.png' } });
  assert.equal(failureOf(missing)._tag, 'AssetWorkspaceAssetNotFoundError');
  assert.match(failureOf(missing).detail, /file was not found/);
  host.stop();
  await assert.rejects(fetch(first.relativeUrl));
});

test('local reply image retention is bounded per conversation with a visible failure', async () => {
  const budget = overrides => mod.host.localImageRetentionFits({
    globalBytes: 0, globalEntries: 0, threadBytes: 0, threadEntries: 0,
    replacedBytes: 0, replacing: false, sizeBytes: 1, ...overrides,
  });
  assert.equal(budget({ globalEntries: 64 }), false, 'global entry limit');
  assert.equal(budget({ threadEntries: 32 }), false, 'conversation entry limit');
  assert.equal(budget({ globalBytes: 128 * 1024 * 1024 }), false, 'global byte limit');
  assert.equal(budget({ threadBytes: 64 * 1024 * 1024 }), false, 'conversation byte limit');
  assert.equal(budget({
    globalBytes: 128 * 1024 * 1024, globalEntries: 64,
    threadBytes: 64 * 1024 * 1024, threadEntries: 32,
    replacedBytes: 20 * 1024 * 1024, replacing: true, sizeBytes: 20 * 1024 * 1024,
  }), true, 'same-size replacement does not grow either budget');
  const engine = new FakeEngine();
  engine.readImageAsset = async ({ path }) => ({
    dataBase64: PNG,
    mimeType: 'image/png',
    sizeBytes: PNG_BYTES,
    sourcePath: `${DEFAULT_FOLDER}/${path}`,
    fileRevision: `revision:${path}`,
  });
  const { host, client } = connect(engine);
  await newThread(client, 'chat-image-budget');
  for (let index = 0; index < 32; index++) {
    const result = await client.call('assets.createUrl', { resource: {
      _tag: 'media-file', threadId: 'chat-image-budget', path: `shots/${index}.png`, hostOnly: true,
    } });
    assert.match(result.relativeUrl, /^blob:/);
  }
  const overflow = await client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-image-budget', path: 'shots/overflow.png', hostOnly: true,
  } });
  assert.equal(failureOf(overflow)._tag, 'AssetWorkspaceAssetNotFoundError');
  assert.match(failureOf(overflow).detail, /too many project images/i);
  host.stop();
});

test('stopping the host while a native image read is pending does not mint a late Blob URL', async () => {
  const engine = new FakeEngine();
  let release;
  engine.readImageAsset = () => new Promise(resolve => { release = resolve; });
  const { host, client } = connect(engine);
  await newThread(client, 'chat-image-stop');
  const pending = client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-image-stop', path: 'shots/late.png', hostOnly: true,
  } });
  while (!release) await new Promise(resolve => setImmediate(resolve));
  const createObjectURL = URL.createObjectURL;
  let minted = 0;
  URL.createObjectURL = value => { minted++; return createObjectURL(value); };
  try {
    host.stop();
    release({
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: `${DEFAULT_FOLDER}/shots/late.png`, fileRevision: 'late',
    });
    const result = await pending;
    assert.equal(failureOf(result)._tag, 'AssetWorkspaceAssetNotFoundError');
    assert.match(failureOf(result).detail, /window has closed/i);
    assert.equal(minted, 0);
  } finally {
    URL.createObjectURL = createObjectURL;
  }
});

test('T3\'s XMLHttpRequest to the HoH address is answered in the window; any other goes to the platform', async () => {
  const http = await built.load('http');
  const calls = [];
  class PlatformXhr extends EventTarget {
    open(method, url) { calls.push(['open', method, String(url)]); }
    setRequestHeader(name, value) { calls.push(['header', name, value]); }
    send(body) { calls.push(['send', body]); }
    abort() { calls.push(['abort']); }
    get status() { return 299; }
    get readyState() { return 1; }
  }
  const target = { XMLHttpRequest: PlatformXhr };
  const received = [];
  const restore = http.installHohXhr(url => new URL(url).host === 'hoh.local', async (...args) => { received.push(args); return { status: 204 }; }, target);
  const xhr = new target.XMLHttpRequest();
  xhr.open('POST', 'http://hoh.local/api/attachments/upload/img-1', true);
  xhr.setRequestHeader('Content-Type', 'image/png');
  const file = new Blob([Buffer.from(PNG, 'base64')], { type: 'image/png' });
  const loaded = new Promise(resolve => xhr.addEventListener('load', resolve));
  xhr.send(file);
  await loaded;
  assert.deepEqual([xhr.status, xhr.readyState], [204, 4]);
  assert.deepEqual(received, [['POST', '/api/attachments/upload/img-1', 'image/png', file]]);
  assert.deepEqual(calls, []);
  const other = new target.XMLHttpRequest();
  other.open('POST', 'https://example.test/upload');
  other.send('x');
  assert.equal(other.status, 299);
  assert.deepEqual(calls.map(call => call[0]), ['open', 'send']);
  restore();
  assert.equal(target.XMLHttpRequest, PlatformXhr);
});
