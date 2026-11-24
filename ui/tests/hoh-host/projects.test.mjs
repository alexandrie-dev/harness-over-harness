// "Add project" in T3 on the HoH host: folders listed through the Tauri host, a folder becomes a T3 project,
// a chat in it works in that folder (the engine's own folder request), and the projects survive a restart.
import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { rm } from 'node:fs/promises';
import { buildHost } from './build.mjs';
import { FakeEngine } from './fake-engine.mjs';
import { WireClient } from './wire-client.mjs';

let built; let mod;
before(async () => {
  built = await buildHost();
  mod = { host: await built.load('host'), wire: await built.load('wire'), socket: await built.load('socket') };
});
after(async () => { await rm(built.dir, { recursive: true, force: true }); });

/** The window's localStorage, as a map that outlives one host. */
class MemoryStorage {
  constructor() { this.map = new Map(); }
  getItem(key) { return this.map.has(key) ? this.map.get(key) : null; }
  setItem(key, value) { this.map.set(key, String(value)); }
}
function connect(engine, storage) {
  const host = new mod.host.HohHost({ engine, throttleMs: 0, storage });
  const client = new WireClient(mod.socket.HohWebSocket, new mod.wire.WireServer(host.handlers()));
  return { host, client };
}
const create = (projectId, workspaceRoot, title = workspaceRoot.split('/').at(-1)) => ({
  type: 'project.create', commandId: 'cmd-' + projectId, projectId, title, workspaceRoot, createWorkspaceRootIfMissing: true, defaultModelSelection: null,
});
const failureOf = exit => exit.cause[0].error;

test('project paths keep trailing spaces and a changed resolved folder refuses the first message', async () => {
  const spaced = '/home/person/Projects/site ';
  const engine = new FakeEngine({ folders: ['/home/person/Documents/HoH', spaced, '/home/person/Projects/site'] });
  const { client } = connect(engine, null);
  assert.equal((await client.call('projects.mutate', create('p-spaced', spaced, 'Spaced'))).workspaceRoot, spaced);
  const request = engine.request.bind(engine);
  engine.request = async (method, params) => method === 'session/working-directory/set'
    ? { cwd: '/home/person/Projects/site' }
    : request(method, params);
  const result = await client.request('orchestration.launchThread', {
    commandId: 'changed-folder', threadId: 'changed-folder', projectId: 'p-spaced', title: 'Test',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'full-access',
    interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'not-sent', text: 'Do not send', attachments: [] },
  });
  assert.match(failureOf(result).message, /folder changed/);
  assert(!engine.requests.some(entry => entry.method === 'session/prompt'));
});

test('the default chat folder can also be explicitly added as a project', async () => {
  const engine = new FakeEngine();
  const storage = new MemoryStorage();
  const { client } = connect(engine, storage);
  await client.call('orchestration.launchThread', {
    commandId: 'scratch', threadId: 'scratch-thread', projectId: 'hoh-project', title: 'Scratch',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' },
  });
  const root = '/home/person/Documents/HoH';
  const project = await client.call('projects.mutate', create('p-default', root));
  assert.equal(project.workspaceRoot, root);
  const again = connect(new FakeEngine({ store: engine.store }), storage);
  const shell = await again.client.stream('orchestration.subscribeShell', {});
  const snapshot = (await shell.until(value => value.kind === 'snapshot')).snapshot;
  assert(snapshot.projects.some(value => value.id === 'p-default' && value.workspaceRoot === root));
});

test('Add project: T3 browses folders through the Tauri host (folders only, hidden ones when asked), without it says so', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine, null);
  const home = await client.call('filesystem.browse', { partialPath: '~/' });
  assert.equal(home.parentPath, '/home/person');
  assert.deepEqual(home.entries.map(entry => entry.name), ['.config', 'Documents', 'Projects']);
  const typed = await client.call('filesystem.browse', { partialPath: '/home/person/Projects/s' });
  assert.deepEqual(typed, { parentPath: '/home/person/Projects', entries: [{ name: 'site', fullPath: '/home/person/Projects/site' }] });
  const missing = await client.request('filesystem.browse', { partialPath: '/nowhere/' });
  assert.deepEqual([failureOf(missing)._tag, failureOf(missing).failure, failureOf(missing).message], ['FilesystemBrowseError', 'read_directory_failed', 'No such folder: /nowhere']);
  // A Tauri host without the folder list: T3 shows the reason, and a typed full path still adds the project.
  const bare = new FakeEngine(); bare.browseFolders = undefined;
  const { client: other } = connect(bare, null);
  const unavailable = await other.request('filesystem.browse', { partialPath: '~/' });
  assert.match(failureOf(unavailable).message, /cannot list folders/);
  assert.equal((await other.call('projects.mutate', create('p-typed', '/home/person/Projects/tools'))).workspaceRoot, '/home/person/Projects/tools');
});

test('a folder becomes a T3 project; a chat in it runs in that folder; the list and the chat\'s place survive a restart', async () => {
  const storage = new MemoryStorage();
  const store = new Map();
  const engine = new FakeEngine({ store });
  const { client } = connect(engine, storage);
  const shell = await client.stream('orchestration.subscribeShell', {});
  const first = await shell.until(value => value.kind === 'snapshot');
  assert.deepEqual(first.snapshot.projects, []);

  const project = await client.call('projects.mutate', create('p-site', '/home/person/Projects/site/', 'site'));
  assert.deepEqual({ id: project.id, title: project.title, root: project.workspaceRoot, deletedAt: project.deletedAt }, { id: 'p-site', title: 'site', root: '/home/person/Projects/site', deletedAt: null });
  assert.equal((await shell.until(value => value.kind === 'project.updated')).project.id, 'p-site');
  // A folder that is not there, a path that is not full, the same folder twice: T3's ProjectMutationError.
  for (const [mutation, words] of [
    [create('p-gone', '/home/person/Gone'), /cannot be opened: No such folder/],
    [create('p-rel', 'Projects/site'), /full path/],
    [create('p-twice', '/home/person/Projects/site'), /already a project/],
  ]) {
    const exit = await client.request('projects.mutate', mutation);
    assert.equal(failureOf(exit)._tag, 'ProjectMutationError'); assert.match(failureOf(exit).message, words);
  }

  const launched = await client.call('orchestration.launchThread', {
    commandId: 'cmd-1', threadId: 'thread-site', projectId: 'p-site', title: 'Fix the build', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'm-1', text: 'Build it', attachments: [] },
  });
  assert.equal(launched.projection.thread.projectId, 'p-site');
  // The folder is set with the current window's request, before the message goes to Claude.
  const order = engine.requests.map(request => request.method).filter(method => method.startsWith('session/working-directory') || method === 'session/prompt');
  assert.deepEqual(order, ['session/working-directory/get', 'session/working-directory/set', 'session/prompt']);
  assert.deepEqual(engine.requests.find(request => request.method === 'session/working-directory/set').params, { sessionId: 'thread-site', path: '/home/person/Projects/site' });
  assert.deepEqual(engine.starts, [{ sessionId: 'thread-site', executor: 'claude', cwd: '/home/person/Projects/site' }]);
  const moved = await client.request('orchestration.launchThread', {
    commandId: 'cmd-move', threadId: 'thread-site', projectId: 'hoh-project', title: 'Move',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'm-move', text: 'Do not send', attachments: [] },
  });
  assert.match(failureOf(moved).message, /cannot change its project folder/);
  const recreated = await client.request('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'cmd-recreate', threadId: 'thread-site', projectId: 'hoh-project', title: 'Move',
  });
  assert.match(failureOf(recreated).message, /cannot change its project folder/);
  assert.equal(engine.requests.filter(request => request.method === 'session/prompt').length, 1);
  assert.equal((await shell.until(value => value.kind === 'thread.updated' && value.thread.id === 'thread-site')).thread.projectId, 'p-site');
  // A chat without a project stays in "No project", the engine's default folder.
  await client.call('orchestration.launchThread', {
    commandId: 'cmd-2', threadId: 'thread-plain', projectId: 'hoh-project', title: 'Plain', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' },
  });
  assert.equal(engine.requests.filter(request => request.method === 'session/working-directory/set').length, 1);
  assert.equal((await client.call('server.getConfig', {})).scratchWorkspaceRoot, '/home/person/Documents/HoH');

  // Restart: a new engine process over the same sessions, a new page over the same storage.
  const restartedEngine = new FakeEngine({ store });
  const { client: again } = connect(restartedEngine, storage);
  const againShell = await again.stream('orchestration.subscribeShell', {});
  const restored = await againShell.until(value => value.kind === 'snapshot');
  assert.deepEqual(restored.snapshot.projects.map(p => [p.id, p.title, p.workspaceRoot]).sort(), [['hoh-project', 'No project', '/home/person/Documents/HoH'], ['p-site', 'site', '/home/person/Projects/site']]);
  assert.deepEqual(Object.fromEntries(restored.snapshot.threads.map(t => [t.id, t.projectId])), { 'thread-site': 'p-site', 'thread-plain': 'hoh-project' });
  assert.equal(store.get('thread-site').cwd, '/home/person/Projects/site');
  await again.call('orchestration.launchThread', {
    commandId: 'cmd-after-restart', threadId: 'thread-site-2', projectId: 'p-site', title: 'After restart',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required',
    interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'm-after-restart', text: 'Build again', attachments: [] },
  });
  assert.deepEqual(restartedEngine.starts, [{ sessionId: 'thread-site-2', executor: 'claude', cwd: '/home/person/Projects/site' }]);
  assert.equal((await again.call('server.getConfig', {})).scratchWorkspaceRoot, '/home/person/Documents/HoH');
  // Opened again, the chat's row takes its title from the history (the engine kept no title here).
  await again.call('orchestration.getThreadProjection', { threadId: 'thread-site' });
  assert.equal((await againShell.until(value => value.kind === 'thread.updated' && value.thread.id === 'thread-site')).thread.title, 'Build it');
});

test('a chat whose history names a project folder joins that project; refusing the folder fails the start in English', async () => {
  const storage = new MemoryStorage();
  const engine = new FakeEngine();
  // A chat begun in the old window, in a folder the person later adds as a project.
  engine.seed('chat-old', 'Old', [['working-directory/change', { cwd: '/home/person/Projects/tools' }], ['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'hi' }] }]]);
  const { client } = connect(engine, storage);
  const shell = await client.stream('orchestration.subscribeShell', {});
  assert.equal((await shell.until(value => value.kind === 'snapshot')).snapshot.threads[0].projectId, 'hoh-project');
  await client.call('projects.mutate', create('p-tools', '/home/person/Projects/tools'));
  await client.call('orchestration.getThreadProjection', { threadId: 'chat-old' });
  assert.equal((await shell.until(value => value.kind === 'thread.updated' && value.thread.id === 'chat-old' && value.thread.projectId === 'p-tools')).thread.projectId, 'p-tools');

  engine.refuseFolder = true;
  const refused = await client.request('orchestration.launchThread', {
    commandId: 'cmd-r', threadId: 'thread-r', projectId: 'p-tools', title: 'R', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' },
  });
  assert.deepEqual([failureOf(refused)._tag, failureOf(refused).message], ['OrchestrationV2ThreadLaunchError', 'You declined this in the HoH confirmation window.']);
  // A typed `~/…` folder (no folder list) becomes the folder the engine resolved once a chat starts in it.
  engine.refuseFolder = false;
  await client.call('projects.mutate', create('p-home', '~/Documents'));
  await client.call('orchestration.launchThread', {
    commandId: 'cmd-h', threadId: 'thread-h', projectId: 'p-home', title: 'H', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' },
  });
  assert.equal((await shell.until(value => value.kind === 'project.updated' && value.project.id === 'p-home' && value.project.workspaceRoot.startsWith('/'))).project.workspaceRoot, '/home/person/Documents');
  // Projects: renamed, pinned; one with chats is kept, an empty one is removed.
  assert.equal((await client.call('projects.mutate', { type: 'project.update', commandId: 'u', projectId: 'p-tools', title: 'Tools', pinned: true })).title, 'Tools');
  assert.match(failureOf(await client.request('projects.mutate', { type: 'project.delete', commandId: 'd', projectId: 'p-tools' })).message, /still has threads/);
  await client.call('projects.mutate', create('p-empty', '/home/person/Projects/site'));
  assert.equal((await client.call('projects.mutate', { type: 'project.delete', commandId: 'd2', projectId: 'p-empty' })).id, 'p-empty');
  assert.equal((await shell.until(value => value.kind === 'project.removed')).projectId, 'p-empty');
});

test('a failed turn and an unsent message are T3\'s English "error" items, never the host\'s Russian words', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-e', 'E', [
    ['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'go' }] }], ['turn/start', { turn: 1 }],
    ['turn/end', { turn: 1, reason: { kind: 'error', error: { message: 'Claude exited with code 1', code: 'UNKNOWN' } } }],
    ['user/message', { id: 'u2', source: { kind: 'user' }, content: [{ type: 'text', text: 'again' }] }], ['turn/start', { turn: 2 }],
    ['turn/end', { turn: 2, reason: { kind: 'error', error: { message: 'Помощник упал', code: 'UNKNOWN' } } }],
  ]);
  const { client } = connect(engine, null);
  const projection = await client.call('orchestration.getThreadProjection', { threadId: 'chat-e' });
  const errors = projection.turnItems.filter(item => item.type === 'error');
  assert.deepEqual(errors.map(item => [item.title, item.status, item.failure.class, item.failure.message]), [
    ['Provider error', 'failed', 'provider_error', 'Claude exited with code 1'],
    ['Provider error', 'failed', 'provider_error', 'Provider turn failed.'],
  ]);
  assert.equal(projection.runs.at(-1).status, 'failed');
  const request = engine.request.bind(engine);
  engine.request = (method, params) => method === 'session/prompt' ? Promise.reject({ code: 'connecting', message: 'Помощник ещё подключается' }) : request(method, params);
  const sent = await client.request('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c1', threadId: 'chat-e', messageId: 'm-x', text: 'hello', attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' } });
  assert.equal(failureOf(sent).message, 'HoH is still starting. Try again in a moment.');
  const after = await client.call('orchestration.getThreadProjection', { threadId: 'chat-e' });
  const unsent = after.turnItems.at(-1);
  assert.deepEqual([unsent.type, unsent.title, unsent.failure.class, unsent.failure.message, unsent.failure.code], ['error', 'Message not sent', 'transport_error', 'HoH is still starting. Try again in a moment.', 'connecting']);
});
