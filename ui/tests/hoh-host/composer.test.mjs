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
const STATIC_PROFILE = { canvasPixels: 4, decodedFramePixels: 4, animationPixelsPerSecond: 0, frameCount: 1 };

function connect(engine, options = {}) {
  const readImageAsset = engine.readImageAsset?.bind(engine);
  if (readImageAsset) engine.readImageAsset = async input => {
    const image = await readImageAsset(input);
    return { decodeProfile: STATIC_PROFILE, ...image };
  };
  const host = new mod.host.HohHost({ engine, throttleMs: 0, storage: null, ...options });
  const client = new WireClient(mod.socket.HohWebSocket, new mod.wire.WireServer(host.handlers()));
  return { host, client };
}
const failureOf = exit => exit.cause[0].error;
const sent = (engine, method) => engine.requests.filter(request => request.method === method);
const upload = (overrides = {}) => ({ type: 'image', id: 'img-1', name: 'red.png', mimeType: 'image/png', sizeBytes: PNG_BYTES, dataUrl: `data:image/png;base64,${PNG}`, ...overrides });
const dispatch = (threadId, messageId, text, attachments = []) => ({
  type: 'message.dispatch', commandId: 'c-' + messageId, threadId, messageId, text, attachments, createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' },
});
const createThread = (threadId, projectId = 'hoh-project') => ({
    type: 'thread.create', commandId: 'c0-' + threadId, threadId, projectId, title: 'T', createdBy: 'user', creationSource: 'web',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default', branch: null, worktreePath: null,
});
async function newThread(client, threadId, projectId = 'hoh-project') {
  await client.call('orchestration.dispatchCommand', createThread(threadId, projectId));
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

test('clipboard tokens are matched by their draft context IDs on launch and follow-up', async t => {
  const engine = new FakeEngine();
  const { host, client } = connect(engine);
  t.after(() => host.stop());
  const threadId = 'chat-clipboard';
  for (const messageId of ['first', 'follow-up']) {
    const { attachments } = await client.call('assets.persistChatAttachments', {
      threadId, messageId, attachments: [upload({ id: 'img-minted-' + messageId })],
    });
    const clipboardRef = '![clipboard.png](t3-context://v1/image/image_draft-' + messageId + ')';
    const unmatched = '![other.png](t3-context://v1/image/image_other)';
    const text = 'Describe this ' + unmatched + '\n' + clipboardRef;
    const context = { version: 1, records: [
      { ...attachments[0], kind: 'image', version: 1, contextId: 'image_draft-' + messageId, attachmentId: attachments[0].id },
      { kind: 'image', contextId: 'image_other', attachmentId: 'unused-upload' },
    ] };
    if (messageId === 'first') {
      await client.call('orchestration.launchThread', {
        commandId: 'launch-clipboard', threadId, title: 'Clipboard', projectId: 'hoh-project',
        modelSelection: { instanceId: 'codex', model: 'gpt-6' },
        initialMessage: { messageId, text, attachments, context },
      });
    } else {
      await client.call('orchestration.dispatchCommand', { ...dispatch(threadId, messageId, text, attachments), context });
    }
    assert.deepEqual(sent(engine, 'session/prompt').at(-1).params.contentBlocks, [
      { type: 'image', data: PNG, mimeType: 'image/png', name: 'red.png' },
      { type: 'text', text: 'Describe this ' + unmatched },
    ]);
  }
  assert.deepEqual(engine.starts.map(start => start.executor), ['codex', 'codex']);
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
  engine.files = [
    { path: 'src', kind: 'directory' },
    { path: 'src/main.ts', kind: 'file' },
    { path: 'logo.png', kind: 'file' },
    { path: '../.env', kind: 'file' },
    { path: 'src/../.env', kind: 'file' },
    { path: '/etc/passwd', kind: 'file' },
    { path: ' /etc/passwd', kind: 'file' },
    { path: 'src/main.ts ', kind: 'file' },
    { path: '\\etc\\passwd', kind: 'file' },
  ];
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
  assert.deepEqual((await client.call('projects.searchEntries', { cwd: SITE, query: '', limit: 80 })).entries, [
    { path: 'src', kind: 'directory' }, { path: 'src/main.ts', kind: 'file' }, { path: 'logo.png', kind: 'file' },
  ]);
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

test('an external image snapshot requires confirmation and is confined to its chat and exact path', async () => {
  const path = '/tmp/hoh-qa/screenshot.png';
  const message = "Image unavailable: the file is outside this conversation's project.";
  const engine = new FakeEngine();
  engine.readImageAsset = async () => { throw new Error(message); };
  let approved = false;
  engine.confirmExternalImage = async input => {
    assert.deepEqual(input, { sessionId: 'chat-approved-image', path });
    return approved ? {
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES, sourcePath: path,
      fileRevision: 'confirmed-image', decodeProfile: STATIC_PROFILE,
    } : null;
  };
  const { host, client } = connect(engine);
  try {
    await newThread(client, 'chat-approved-image');
    await newThread(client, 'chat-other-image');
    const read = (threadId, requested = path) => client.call('assets.createUrl', {
      resource: { _tag: 'media-file', threadId, path: requested },
    });
    await assert.rejects(read('chat-approved-image'));
    assert.equal(await host.showExternalImage('chat-approved-image', path), false);
    await assert.rejects(read('chat-approved-image'));
    approved = true;
    assert.equal(await host.showExternalImage('chat-approved-image', path), true);
    const preview = await read('chat-approved-image');
    assert.match(preview.relativeUrl, /^blob:/);
    assert.equal((await fetch(preview.relativeUrl)).headers.get('content-type'), 'image/png');
    await assert.rejects(read('chat-other-image'));
    await assert.rejects(read('chat-approved-image', path.replace('screenshot', 'other')));
  } finally {
    host.stop();
  }
});

test('a new conversation accepts its first-turn image before subscribe/create, then rebases it to the project folder', async () => {
  const engine = new FakeEngine();
  const originalRequest = engine.request.bind(engine);
  let announceDirectory;
  const directoryStarted = new Promise(resolve => { announceDirectory = resolve; });
  let releaseDirectory;
  const directoryGate = new Promise(resolve => { releaseDirectory = resolve; });
  const threadId = 'chat-first-local-image';
  engine.request = async (method, params) => {
    if (method === 'session/working-directory/get' && params.sessionId === threadId) {
      announceDirectory();
      await directoryGate;
    }
    return originalRequest(method, params);
  };
  const reads = [];
  engine.readImageAsset = async input => {
    reads.push(input);
    return {
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: `${SITE}/shots/test-image.png`, fileRevision: 'first-turn',
    };
  };
  const { host, client } = connect(engine);
  await siteProject(client);
  let imageSettled = false;
  const image = client.call('assets.createUrl', { resource: {
    _tag: 'media-file', threadId, path: '~/Documents/HoH/shots/test-image.png', hostOnly: true,
  } }).finally(() => { imageSettled = true; });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(host.threadSinks.has(threadId), false, 'the media query really precedes draft subscription');
  assert.equal(host.creatingThreads.has(threadId), false, 'the media query really precedes thread.create');
  assert.equal(host.localImageContextWaiters.has(threadId), true);
  const thread = await client.stream('orchestration.subscribeThread', { threadId });
  const creating = client.call('orchestration.dispatchCommand', createThread(threadId, 'p-site'));
  await directoryStarted;
  assert.equal(imageSettled, false, 'the optimistic render remains loading while the folder is provisional');
  assert.equal(reads.length, 0, 'no native image read starts before the authoritative folder exists');
  assert.equal(host.localImageAttempts.has(threadId), false);
  assert.equal(host.localImageReservations.size, 0);
  assert.equal(host.localImageFailures.size, 0);

  releaseDirectory();
  await creating;
  await client.call('orchestration.dispatchCommand', dispatch(
    threadId, 'm-first-local-image', '![alt](shots/test-image.png)',
  ));
  const minted = await image;
  assert.match(minted.relativeUrl, /^blob:/);
  assert.deepEqual(reads, [{ sessionId: threadId, path: `${SITE}/shots/test-image.png` }]);
  assert.equal(host.localImageAttempts.get(threadId).length, 1);
  assert.equal(host.localImageFailures.size, 0);
  client.interrupt(thread);
  host.stop();
});

test('an unregistered image context expires without native reads, cache, or budget use', async () => {
  const engine = new FakeEngine();
  let reads = 0;
  engine.readImageAsset = async () => { reads++; throw new Error('must not read'); };
  const { host, client } = connect(engine, { localImageContextGraceMs: 10 });
  const refused = await client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'not-a-draft', path: 'shots/test-image.png', hostOnly: true,
  } });
  assert.equal(failureOf(refused)._tag, 'AssetWorkspaceContextNotFoundError');
  assert.equal(reads, 0);
  assert.equal(host.localImageContextWaiters.size, 0);
  assert.equal(host.localImageContextInflight.size, 0);
  assert.equal(host.localImageAttempts.size, 0);
  assert.equal(host.localImageFailures.size, 0);
  host.stop();
});

test('a first image keeps its pre-registration identity while slow startup admits subscribe/create', async () => {
  const engine = new FakeEngine();
  const originalRequest = engine.request.bind(engine);
  let announceList;
  const listStarted = new Promise(resolve => { announceList = resolve; });
  let releaseList;
  const listGate = new Promise(resolve => { releaseList = resolve; });
  engine.request = async (method, params) => {
    if (method === 'session/list') {
      announceList();
      await listGate;
    }
    return originalRequest(method, params);
  };
  const reads = [];
  engine.readImageAsset = async input => {
    reads.push(input);
    return {
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: `${DEFAULT_FOLDER}/shots/test-image.png`, fileRevision: 'slow-start',
    };
  };
  const { host, client } = connect(engine);
  const threadId = 'chat-slow-start-image';
  const image = client.call('assets.createUrl', { resource: {
    _tag: 'media-file', threadId, path: '~/Documents/HoH/shots/test-image.png', hostOnly: true,
  } });
  await listStarted;
  const stream = await client.stream('orchestration.subscribeThread', { threadId });
  const creating = client.call('orchestration.dispatchCommand', createThread(threadId));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(host.threadSinks.has(threadId), true);
  assert.equal(reads.length, 0);
  releaseList();
  await creating;
  assert.match((await image).relativeUrl, /^blob:/);
  assert.deepEqual(reads, [{ sessionId: threadId, path: `${DEFAULT_FOLDER}/shots/test-image.png` }]);
  client.interrupt(stream);
  host.stop();
});

test('an existing nested project thread keeps its already-authoritative hostOnly path', async () => {
  const engine = new FakeEngine();
  engine.cwd = SITE;
  const reads = [];
  engine.readImageAsset = async input => {
    reads.push(input);
    return {
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: input.path, fileRevision: 'nested-project',
    };
  };
  const { host, client } = connect(engine);
  await newThread(client, 'chat-default-root');
  engine.folders.add(`${SITE}/nested`);
  await client.call('projects.mutate', {
    type: 'project.create', commandId: 'cp-nested', projectId: 'p-nested', title: 'nested',
    workspaceRoot: `${SITE}/nested`, createWorkspaceRootIfMissing: false, defaultModelSelection: null,
  });
  await newThread(client, 'chat-nested', 'p-nested');
  await client.call('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-nested', path: `${SITE}/nested/shots/test-image.png`, hostOnly: true,
  } });
  assert.deepEqual(reads, [{ sessionId: 'chat-nested', path: `${SITE}/nested/shots/test-image.png` }]);
  host.stop();
});

test('draft image waiters end on create failure, last unsubscribe, and host stop without native reads', async () => {
  const within = promise => Promise.race([
    promise,
    new Promise((_, reject) => setTimeout(() => reject(new Error('draft image request did not settle')), 250)),
  ]);
  const engine = new FakeEngine();
  let reads = 0;
  engine.readImageAsset = async () => { reads++; throw new Error('must not read'); };
  const originalRequest = engine.request.bind(engine);
  let releaseFailure;
  const failureGate = new Promise(resolve => { releaseFailure = resolve; });
  let announceFailure;
  const failureStarted = new Promise(resolve => { announceFailure = resolve; });
  engine.request = async (method, params) => {
    if (method === 'session/working-directory/get' && params.sessionId === 'chat-create-fails') {
      announceFailure();
      await failureGate;
      throw new Error('folder creation failed');
    }
    if (method === 'session/working-directory/get' && params.sessionId === 'chat-created-without-folder') {
      await originalRequest(method, params);
      return { cwd: null };
    }
    return originalRequest(method, params);
  };
  const { host, client } = connect(engine);
  const failedStream = await client.stream('orchestration.subscribeThread', { threadId: 'chat-create-fails' });
  const creation = client.request('orchestration.dispatchCommand', createThread('chat-create-fails'));
  await failureStarted;
  const waiting = client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-create-fails', path: 'shots/test-image.png', hostOnly: true,
  } });
  releaseFailure();
  assert.equal((await creation)._tag, 'Failure');
  assert.equal(failureOf(await within(waiting))._tag, 'AssetWorkspaceContextNotFoundError');
  assert.equal(host.localImageContextWaiters.size, 0);
  assert.equal(host.failedLocalImageContexts.has('chat-create-fails'), true);
  const late = await within(client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-create-fails', path: 'shots/late.png', hostOnly: true,
  } }));
  assert.equal(failureOf(late)._tag, 'AssetWorkspaceContextNotFoundError');
  assert.equal(reads, 0);
  client.interrupt(failedStream);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(host.failedLocalImageContexts.has('chat-create-fails'), false);

  const unsubscribed = await client.stream('orchestration.subscribeThread', { threadId: 'chat-draft-closes' });
  const closes = client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-draft-closes', path: 'shots/test-image.png', hostOnly: true,
  } });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(host.localImageContextWaiters.has('chat-draft-closes'), true);
  client.interrupt(unsubscribed);
  assert.equal(failureOf(await within(closes))._tag, 'AssetWorkspaceContextNotFoundError');
  assert.equal(host.localImageContextWaiters.has('chat-draft-closes'), false);

  const noFolderStream = await client.stream('orchestration.subscribeThread', { threadId: 'chat-created-without-folder' });
  await client.call('orchestration.dispatchCommand', createThread('chat-created-without-folder'));
  const noFolder = client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-created-without-folder', path: 'shots/test-image.png', hostOnly: true,
  } });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(host.localImageContextWaiters.has('chat-created-without-folder'), true);
  client.interrupt(noFolderStream);
  assert.equal(failureOf(await within(noFolder))._tag, 'AssetWorkspaceContextNotFoundError');
  assert.equal(host.localImageContextWaiters.has('chat-created-without-folder'), false);
  assert.equal(host.threadSinks.has('chat-created-without-folder'), false);
  const noFolderLate = await within(client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-created-without-folder', path: 'shots/late.png', hostOnly: true,
  } }));
  assert.equal(failureOf(noFolderLate)._tag, 'AssetWorkspaceContextNotFoundError');

  await client.stream('orchestration.subscribeThread', { threadId: 'chat-window-closes' });
  const stops = client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-window-closes', path: 'shots/test-image.png', hostOnly: true,
  } });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(host.localImageContextWaiters.has('chat-window-closes'), true);
  host.stop();
  assert.equal(failureOf(await within(stops))._tag, 'AssetWorkspaceContextNotFoundError');
  assert.equal(host.localImageContextWaiters.size, 0);
  assert.equal(reads, 0);
});

test('draft image context lookups deduplicate identical paths and bound distinct waiters', async () => {
  const engine = new FakeEngine();
  let reads = 0;
  engine.readImageAsset = async () => { reads++; throw new Error('must not read'); };
  const { host, client } = connect(engine);
  const threadId = 'chat-bounded-draft-images';
  const stream = await client.stream('orchestration.subscribeThread', { threadId });
  const request = path => client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId, path, hostOnly: true,
  } });
  const first = request('shots/one.png');
  const duplicate = request('shots/one.png');
  const second = request('shots/two.png');
  const third = request('shots/three.png');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(host.localImageContextInflight.size, 3);
  assert.equal(host.localImageContextWaiters.get(threadId).size, 3);
  const overflow = await request('shots/four.png');
  assert.equal(failureOf(overflow)._tag, 'AssetWorkspaceAssetNotFoundError');
  assert.match(failureOf(overflow).detail, /too many project images are waiting/i);
  assert.equal(host.localImageContextInflight.size, 3);
  client.interrupt(stream);
  for (const result of await Promise.all([first, duplicate, second, third])) {
    assert.equal(failureOf(result)._tag, 'AssetWorkspaceContextNotFoundError');
  }
  assert.equal(host.localImageContextWaiters.size, 0);
  assert.equal(host.localImageContextInflight.size, 0);
  assert.equal(reads, 0);
  host.stop();
});

test('one transient unknown-folder read is refunded; a persistent one is counted and cached', async () => {
  const engine = new FakeEngine();
  const reads = new Map();
  let emptyFolder = false;
  const originalRequest = engine.request.bind(engine);
  engine.request = async (method, params) => method === 'session/working-directory/get'
    && params.sessionId === 'chat-folder-transient' && emptyFolder
    ? { cwd: null }
    : originalRequest(method, params);
  engine.readImageAsset = async ({ path }) => {
    const count = (reads.get(path) ?? 0) + 1;
    reads.set(path, count);
    if (path === 'shots/once.png' && count > 1) return {
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: `${DEFAULT_FOLDER}/${path}`, fileRevision: 'ready',
    };
    if (path === 'shots/no-folder.png') emptyFolder = true;
    throw new Error('Image unavailable: the conversation project folder is unknown.');
  };
  const { host, client } = connect(engine);
  const threadId = 'chat-folder-transient';
  await newThread(client, threadId);
  const resource = path => ({ _tag: 'media-file', threadId, path, hostOnly: true });
  const once = await client.call('assets.createUrl', { resource: resource('shots/once.png') });
  assert.match(once.relativeUrl, /^blob:/);
  assert.equal(reads.get('shots/once.png'), 2);
  assert.equal(host.localImageAttempts.get(threadId).length, 1, 'only the successful retry is charged');
  assert.equal(host.localImageFailures.size, 0);

  const persistent = await client.request('assets.createUrl', { resource: resource('shots/persistent.png') });
  assert.match(failureOf(persistent).detail, /project folder is unknown/i);
  assert.equal(reads.get('shots/persistent.png'), 2);
  assert.equal(host.localImageAttempts.get(threadId).length, 2, 'the second consecutive transient is charged');
  assert.equal(host.localImageFailures.size, 1);
  await client.request('assets.createUrl', { resource: resource('shots/persistent.png') });
  assert.equal(reads.get('shots/persistent.png'), 2, 'the persistent error uses the bounded negative cache');
  assert.equal(host.localImageAttempts.get(threadId).length, 2);

  const noFolder = await client.request('assets.createUrl', { resource: resource('shots/no-folder.png') });
  assert.match(failureOf(noFolder).detail, /project folder is unknown/i);
  assert.equal(reads.get('shots/no-folder.png'), 1, 'an absent refreshed cwd does not enter an unbounded retry');
  assert.equal(host.localImageAttempts.get(threadId).length, 3, 'the failed native read remains charged');
  await client.request('assets.createUrl', { resource: resource('shots/no-folder.png') });
  assert.equal(reads.get('shots/no-folder.png'), 1, 'the persistent error is cached negatively');
  host.stop();
});

test('a local reply image is minted only for its conversation and native failures keep a visible reason', async () => {
  const engine = new FakeEngine();
  const reads = [];
  let now = 1_000_000;
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
  const { host, client } = connect(engine, { now: () => now });
  await newThread(client, 'chat-local');
  const resource = { _tag: 'media-file', threadId: 'chat-local', path: 'shots/red.png', hostOnly: true };
  const first = await client.call('assets.createUrl', { resource });
  assert.match(first.relativeUrl, /^blob:/);
  assert.equal(first.sourcePath, `${DEFAULT_FOLDER}/shots/red.png`);
  assert.equal(first.fileRevision, 'dev:ino:size:time');
  assert.equal(Buffer.from(await (await fetch(first.relativeUrl)).arrayBuffer()).toString('base64'), PNG);
  // A remint after the reuse window revalidates, preserving the live URL for the same revision.
  now += 30_001;
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
  const missingAgain = await client.request('assets.createUrl', { resource: { ...resource, path: 'missing.png' } });
  assert.match(failureOf(missingAgain).detail, /file was not found/);
  assert.equal(reads.length, 4, 'a native failure is cached negatively');
  host.stop();
  await assert.rejects(fetch(first.relativeUrl));
});

test('repeated previews reuse an image without spending the budget for new files', async () => {
  const engine = new FakeEngine();
  let now = 1_000_000;
  let reads = 0;
  let revision = 'initial';
  engine.readImageAsset = async ({ path }) => {
    reads++;
    return { dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: `${DEFAULT_FOLDER}/${path}`, fileRevision: revision };
  };
  const { host, client } = connect(engine, { now: () => now });
  await newThread(client, 'chat-preview-reuse');
  const call = path => client.call('assets.createUrl', {
    resource: { _tag: 'media-file', threadId: 'chat-preview-reuse', path, hostOnly: true },
  });
  try {
    const initial = await call('shots/repeated.png');
    for (let index = 0; index < 80; index++) {
      now += 100;
      assert.equal((await call('shots/repeated.png')).relativeUrl, initial.relativeUrl);
    }
    assert.equal(reads, 1);
    assert.notEqual((await call('shots/new.png')).relativeUrl, initial.relativeUrl);
    assert.equal(reads, 2);
    now += 30_001;
    revision = 'updated';
    const updated = await call('shots/repeated.png');
    assert.equal(updated.fileRevision, revision);
    assert.notEqual(updated.relativeUrl, initial.relativeUrl);
    assert.equal(reads, 3);
  } finally {
    host.stop();
  }
});

test('local reply image retention is bounded per conversation with a visible failure', async () => {
  let now = 1_000_000;
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
  let reads = 0;
  engine.readImageAsset = async ({ path, sessionId }) => {
    reads++;
    const sourcePath = path.startsWith('aliases/foreign/')
      ? `${DEFAULT_FOLDER}/shots/foreign-${sessionId}.png`
      : `${DEFAULT_FOLDER}/${path}`;
    return {
      dataBase64: PNG,
      mimeType: 'image/png',
      sizeBytes: PNG_BYTES,
      sourcePath,
      fileRevision: `revision:${sourcePath}`,
    };
  };
  const { host, client } = connect(engine, { now: () => now });
  await newThread(client, 'chat-image-budget');
  let firstAtCapacity;
  for (let index = 0; index < 32; index++) {
    const result = await client.call('assets.createUrl', { resource: {
      _tag: 'media-file', threadId: 'chat-image-budget', path: `shots/${index}.png`, hostOnly: true,
    } });
    assert.match(result.relativeUrl, /^blob:/);
    if (index === 0) firstAtCapacity = result;
  }
  const overflow = await client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-image-budget', path: 'shots/overflow.png', hostOnly: true,
  } });
  assert.equal(failureOf(overflow)._tag, 'AssetWorkspaceAssetNotFoundError');
  assert.match(failureOf(overflow).detail, /too many project images/i);
  assert.equal(reads, 32, 'the retention budget is enforced before native I/O');
  for (let thread = 0; thread < 4; thread++) {
    const threadId = `chat-image-foreign-${thread}`;
    await newThread(client, threadId);
    for (let alias = 0; alias < 50; alias++) {
      await client.call('assets.createUrl', { resource: {
        _tag: 'media-file', threadId,
        path: `aliases/foreign/${thread}/${alias}.png`, hostOnly: true,
      } });
    }
  }
  assert.equal(reads, 232, '200 aliases from other conversations complete within their own identity sets');
  now += 30_001;
  const revalidateAtCapacity = await client.call('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-image-budget', path: 'shots/0.png', hostOnly: true,
  } });
  assert.equal(revalidateAtCapacity.relativeUrl, firstAtCapacity.relativeUrl);
  assert.equal(reads, 233, 'foreign alias churn cannot evict a retained conversation identity');
  host.stop();

  const byteEngine = new FakeEngine();
  let byteReads = 0;
  byteEngine.readImageAsset = async () => {
    byteReads++;
    return {
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: `${DEFAULT_FOLDER}/shots/0.png`, fileRevision: 'new',
    };
  };
  const { host: byteHost, client: byteClient } = connect(byteEngine);
  const byteThread = 'chat-image-byte-budget';
  await newThread(byteClient, byteThread);
  for (let index = 0; index < 4; index++) {
    const sourcePath = `${DEFAULT_FOLDER}/shots/${index}.png`;
    byteHost.localImageUrls.set(`${byteThread}\0${sourcePath}`, {
      url: `blob:retained-${index}`, revision: 'old', sourcePath,
      sizeBytes: 16 * 1024 * 1024, threadId: byteThread,
      decodeProfile: STATIC_PROFILE,
      requestKeys: new Map(index === 0 ? [[`${byteThread}\0shots/0.png`, null]] : []),
    });
  }
  byteHost.localImageBytes = 64 * 1024 * 1024;
  const byteRefresh = await byteClient.call('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: byteThread, path: 'shots/0.png', hostOnly: true,
  } });
  assert.match(byteRefresh.relativeUrl, /^blob:/);
  assert.equal(byteReads, 1, 'a retained key refreshes without speculative byte growth at the byte cap');
  byteHost.stop();
});

test('verified decoded and animation costs are retained before minting another project-image Blob', async () => {
  const zero = { canvasPixels: 0, decodedFramePixels: 0, animationPixelsPerSecond: 0, frameCount: 0 };
  const one = { canvasPixels: 1, decodedFramePixels: 1, animationPixelsPerSecond: 1, frameCount: 2 };
  assert.equal(mod.host.localImageDecodeRetentionFits({
    global: zero,
    thread: { ...zero, canvasPixels: 80_000_000 },
    replaced: zero, incoming: one,
  }), false, 'conversation canvas budget');
  assert.equal(mod.host.localImageDecodeRetentionFits({
    global: zero,
    thread: { ...zero, decodedFramePixels: 200_000_000 },
    replaced: zero, incoming: one,
  }), false, 'conversation aggregate frame-pixel budget');
  assert.equal(mod.host.localImageDecodeRetentionFits({
    global: zero,
    thread: { ...zero, animationPixelsPerSecond: 200_000_000 },
    replaced: zero, incoming: one,
  }), false, 'conversation animation-rate budget');

  const engine = new FakeEngine();
  let reads = 0;
  engine.readImageAsset = async ({ path, sessionId }) => ({
    dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
    sourcePath: `${DEFAULT_FOLDER}/${sessionId}/${path}`, fileRevision: `revision:${sessionId}:${path}`,
    decodeProfile: { canvasPixels: 40_000_000, decodedFramePixels: 40_000_000, animationPixelsPerSecond: 0, frameCount: 1 },
    readNumber: ++reads,
  });
  const { host, client } = connect(engine);
  for (const threadId of ['decode-a', 'decode-b', 'decode-c']) await newThread(client, threadId);
  let minted = 0;
  const createObjectURL = URL.createObjectURL;
  URL.createObjectURL = blob => { minted++; return createObjectURL(blob); };
  try {
    const payload = (threadId, path) => ({ resource: {
      _tag: 'media-file', threadId, path, hostOnly: true,
    } });
    const open = (threadId, path) => client.call('assets.createUrl', payload(threadId, path));
    const request = (threadId, path) => client.request('assets.createUrl', payload(threadId, path));
    for (const path of ['a.png', 'b.png']) assert.match((await open('decode-a', path)).relativeUrl, /^blob:/);
    const threadOverflow = await request('decode-a', 'c.png');
    assert.match(failureOf(threadOverflow).detail, /decoded-image or animation budget/i);
    for (const path of ['a.png', 'b.png']) assert.match((await open('decode-b', path)).relativeUrl, /^blob:/);
    const globalOverflow = await request('decode-c', 'a.png');
    assert.match(failureOf(globalOverflow).detail, /decoded-image or animation budget/i);
    assert.equal(minted, 4, 'over-budget native responses are rejected before Blob URL minting');
    assert.equal(reads, 6, 'compressed-byte and native-read budgets remain independent');
  } finally {
    URL.createObjectURL = createObjectURL;
    host.stop();
  }
});

test('identical local image reads in flight are combined and failure history is a bounded LRU', async () => {
  const engine = new FakeEngine();
  let release;
  let reads = 0;
  engine.readImageAsset = ({ path }) => {
    reads++;
    if (path === 'shots/shared.png') return new Promise(resolve => { release = resolve; });
    throw new Error(`Image unavailable: ${path} was not found.`);
  };
  const { host, client } = connect(engine);
  const threadIds = ['chat-image-cache', 'chat-image-cache-2', 'chat-image-cache-3'];
  for (const threadId of threadIds) await newThread(client, threadId);
  const resource = (path, threadId = threadIds[0]) => ({ _tag: 'media-file', threadId, path, hostOnly: true });
  const first = client.call('assets.createUrl', { resource: resource('shots/shared.png') });
  const second = client.call('assets.createUrl', { resource: resource('shots/shared.png') });
  while (!release) await new Promise(resolve => setImmediate(resolve));
  assert.equal(reads, 1, 'only one native read starts for an identical in-flight request');
  release({
    dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
    sourcePath: `${DEFAULT_FOLDER}/shots/shared.png`, fileRevision: 'shared',
  });
  assert.equal((await first).relativeUrl, (await second).relativeUrl);

  const failed = (path, threadId) => client.request('assets.createUrl', { resource: resource(path, threadId) });
  await failed('missing/first.png');
  await failed('missing/first.png');
  assert.equal(reads, 2, 'a repeated failure uses the negative cache');
  for (let index = 0; index < 128; index++) await failed(`missing/${index}.png`, threadIds[index % threadIds.length]);
  assert.equal(reads, 130);
  await failed('missing/first.png');
  assert.equal(reads, 131, 'the oldest failure is evicted from the 128-entry LRU');
  host.stop();
});

test('a burst of project images waits for read slots and loads without reopening the chat', async () => {
  const engine = new FakeEngine();
  const firstWave = [];
  const reads = [];
  engine.readImageAsset = async ({ sessionId, path }) => {
    reads.push({ sessionId, path });
    if (reads.length <= 6) await new Promise(resolve => firstWave.push(resolve));
    if (path === 'shots/missing.png') throw new Error('Image unavailable: the file was not found.');
    return {
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: `${DEFAULT_FOLDER}/${path}`, fileRevision: path,
    };
  };
  const { host, client } = connect(engine);
  const threadIds = ['chat-image-burst-a', 'chat-image-burst-b', 'chat-image-burst-c'];
  for (const threadId of threadIds) await newThread(client, threadId);
  const request = (threadId, path) => client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId, path, hostOnly: true,
  } });
  try {
    const missing = request(threadIds[0], 'shots/missing.png');
    const images = threadIds.flatMap(threadId => Array.from({ length: 4 }, (_, index) =>
      request(threadId, `shots/${index}.png`)));
    const duplicate = request(threadIds[0], 'shots/3.png');
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(reads.map(read => read.sessionId), [
      ...Array(3).fill(threadIds[0]), ...Array(3).fill(threadIds[1]),
    ]);

    for (const release of firstWave) release();
    assert.match(failureOf(await missing).detail, /file was not found/i);
    const results = await Promise.all(images);
    for (const result of results) assert.match(result.value.relativeUrl, /^blob:/);
    assert.equal((await duplicate).value.relativeUrl, results[3].value.relativeUrl);
    assert.equal(reads.length, 13, 'the queued duplicate shares its native read');
  } finally {
    for (const release of firstWave) release();
    host.stop();
  }
});

test('retained raw image identity slots are reserved before concurrent native reads', async () => {
  const engine = new FakeEngine();
  let reads = 0;
  let now = 2_000_000;
  let releaseConcurrent;
  engine.readImageAsset = async ({ path }) => {
    reads++;
    if (path.startsWith('concurrent/')) {
      return new Promise(resolve => { releaseConcurrent = () => resolve({
        dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
        sourcePath: `${DEFAULT_FOLDER}/shots/0.png`, fileRevision: 'revision:0',
      }); });
    }
    const canonical = path.startsWith('aliases/') ? 'shots/0.png' : path;
    return {
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: `${DEFAULT_FOLDER}/${canonical}`,
      fileRevision: canonical === 'shots/0.png' ? 'revision:0' : `revision:${canonical}`,
    };
  };
  const { host, client } = connect(engine, { now: () => now });
  const threadId = 'chat-image-identities';
  await newThread(client, threadId);
  const payload = path => ({ resource: {
    _tag: 'media-file', threadId, path, hostOnly: true,
  } });
  let attemptsInWindow = 0;
  const read = async path => {
    if (attemptsInWindow === 64) {
      now += 10 * 60_000 + 1;
      attemptsInWindow = 0;
    }
    const result = await client.call('assets.createUrl', payload(path));
    attemptsInWindow++;
    return result;
  };

  const original = await read('shots/0.png');
  for (let index = 0; index < 254; index++) await read(`aliases/${index}.png`);
  assert.equal(reads, 255);

  const admitted = client.call('assets.createUrl', payload('concurrent/admitted.png'));
  while (!releaseConcurrent) await new Promise(resolve => setImmediate(resolve));
  const blockedA = client.request('assets.createUrl', payload('concurrent/blocked-a.png'));
  const blockedB = client.request('assets.createUrl', payload('concurrent/blocked-b.png'));
  assert.equal(reads, 256, 'the final identity slot is reserved before native I/O');
  releaseConcurrent();
  await admitted;
  for (const blocked of [await blockedA, await blockedB]) {
    assert.match(failureOf(blocked).detail, /too many project image paths/i);
  }
  assert.equal(reads, 256);

  const overCap = await client.request('assets.createUrl', payload('aliases/over-cap.png'));
  assert.match(failureOf(overCap).detail, /too many project image paths/i);
  assert.equal(reads, 256, 'an unknown identity over the lifetime cap is refused before native I/O');

  now += 10 * 60_000 + 1;
  const refreshed = await client.call('assets.createUrl', payload('shots/0.png'));
  assert.equal(refreshed.relativeUrl, original.relativeUrl);
  assert.equal(reads, 257, 'an admitted original identity still refreshes at the identity cap');
  host.stop();
});

test('native image attempts are bounded per conversation across failures and aliases', async () => {
  const engine = new FakeEngine();
  let reads = 0;
  let now = 1_000_000;
  engine.readImageAsset = async ({ path }) => {
    reads++;
    if (path.startsWith('missing/')) throw new Error('Image unavailable: missing.');
    return {
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: `${DEFAULT_FOLDER}/shots/shared.png`, fileRevision: 'shared',
    };
  };
  const { host, client } = connect(engine, { now: () => now });
  await newThread(client, 'chat-image-attempts');
  const payload = (path, threadId = 'chat-image-attempts') => ({ resource: {
    _tag: 'media-file', threadId, path, hostOnly: true,
  } });
  const request = path => client.request('assets.createUrl', payload(path));
  const call = path => client.call('assets.createUrl', payload(path));
  for (let index = 0; index < 4; index++) await request(`missing/${index}.png`);
  for (let index = 0; index < 60; index++) {
    const result = await call(`aliases/${index}.png`);
    assert.match(result.relativeUrl, /^blob:/);
  }
  assert.equal(reads, 64);
  const refused = await request('aliases/blocked.png');
  assert.match(failureOf(refused).detail, /too many project image reads/i);
  assert.equal(reads, 64, 'the exhausted attempt budget refuses before native I/O');

  now += 30_001;
  assert.match((await call('aliases/0.png')).relativeUrl, /^blob:/);
  assert.equal(reads, 64, 'an already opened image stays visible while the quota is exhausted');

  for (let index = 0; index < 128; index++) {
    const threadId = `chat-image-churn-${index}`;
    await newThread(client, threadId);
    await client.request('assets.createUrl', payload(`missing/churn-${index}.png`, threadId));
  }
  assert.equal(reads, 192);
  const stillRefused = await request('aliases/blocked-after-churn.png');
  assert.match(failureOf(stillRefused).detail, /too many project image reads/i);
  assert.equal(reads, 192, 'active attempt history survives churn from 128 other conversations');

  now += 10 * 60_000 + 1;
  const afterWindow = await call('aliases/after-window.png');
  assert.match(afterWindow.relativeUrl, /^blob:/);
  assert.equal(reads, 193);
  host.stop();
});

test('stopping the host cancels queued image reads and prevents late Blob URLs', async () => {
  const engine = new FakeEngine();
  const releases = [];
  engine.readImageAsset = () => new Promise(resolve => { releases.push(resolve); });
  const { host, client } = connect(engine);
  await newThread(client, 'chat-image-stop');
  const request = index => client.request('assets.createUrl', { resource: {
    _tag: 'media-file', threadId: 'chat-image-stop', path: `shots/late-${index}.png`, hostOnly: true,
  } });
  const pending = [request(0), request(1), request(2)];
  const queued = request(3);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(releases.length, 3);
  const createObjectURL = URL.createObjectURL;
  let minted = 0;
  URL.createObjectURL = value => { minted++; return createObjectURL(value); };
  try {
    host.stop();
    for (const release of releases) release({
      dataBase64: PNG, mimeType: 'image/png', sizeBytes: PNG_BYTES,
      sourcePath: `${DEFAULT_FOLDER}/shots/late.png`, fileRevision: 'late',
    });
    for (const result of await Promise.all([...pending, queued])) {
      assert.equal(failureOf(result)._tag, 'AssetWorkspaceAssetNotFoundError');
      assert.match(failureOf(result).detail, /window has closed/i);
    }
    assert.equal(releases.length, 3, 'the queued request never reaches the native bridge');
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
